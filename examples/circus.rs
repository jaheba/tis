//! Benchmark-only JSON worker for Flying Circus. Fresh sessions retain no VM/JIT state.
use serde_json::{Value as Json, json};
use std::{
    env, fs,
    io::{self, BufRead, Write},
};
use tis::{Tiering, Vm, compile};

#[derive(Clone, Copy)]
struct Options {
    tier: Tiering,
    readonly: bool,
    mutation: bool,
    diagnostics: bool,
}
impl Options {
    fn vm(self) -> Vm {
        let mut vm = Vm::new();
        vm.tiering = self.tier;
        vm.readonly_regions = self.readonly;
        vm.mutation_regions = self.mutation;
        vm
    }
}
fn execute(vm: &mut Vm, code: &str, diagnostics: bool) -> Json {
    vm.output.clear();
    let result = compile(code).and_then(|mut program| vm.run_program(&mut program));
    let mut response = match result {
        Ok(_) => json!({"ok":true,"stdout":vm.output,"stderr":""}),
        Err(error) => json!({"ok":false,"exc_type":match &error {
            tis::Error::Unsupported(_)=>"NotImplementedError",tis::Error::Syntax(_)=>"SyntaxError",_=>"RuntimeError"
        },"error":error.to_string()}),
    };
    if diagnostics {
        let stats = &vm.tier_stats;
        response["stats"] = json!({"native_entries":stats.native_entries,"compilation_errors":stats.compilation_errors,
            "readonly_entries":stats.readonly_entries,"mutation_entries":stats.mutation_entries});
    }
    response
}
fn worker(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut vm = options.vm();
    println!("{{\"ready\":true}}");
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let request: Json = serde_json::from_str(&line?)?;
        let response = match request["kind"].as_str() {
            Some("reset") => {
                vm = options.vm();
                json!({"ok":true})
            }
            Some("run") => execute(
                &mut vm,
                request["code"].as_str().ok_or("missing code")?,
                options.diagnostics,
            ),
            _ => json!({"ok":false,"exc_type":"ValueError","error":"unknown worker request"}),
        };
        println!("{response}");
        io::stdout().flush()?;
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options {
        tier: Tiering::Bytecode,
        readonly: false,
        mutation: false,
        diagnostics: false,
    };
    let mut args = env::args().skip(1);
    let mut source = None;
    let mut serve = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--version" => {
                println!("tis-bench {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--tier" => {
                options.tier = match args.next().as_deref() {
                    Some("native") => Tiering::Native,
                    Some("bytecode") => Tiering::Bytecode,
                    Some("lir") => Tiering::Lir,
                    Some("copy-patch") => Tiering::CopyPatch,
                    _ => return Err("invalid tier".into()),
                }
            }
            "--readonly-regions" => options.readonly = true,
            "--mutation-regions" => options.mutation = true,
            "--diagnostics" => options.diagnostics = true,
            "--worker" => serve = true,
            "-c" => source = Some(args.next().ok_or("missing code")?),
            path if source.is_none() => source = Some(fs::read_to_string(path)?),
            _ => return Err("unexpected argument".into()),
        }
    }
    if serve {
        return worker(options);
    }
    let mut vm = options.vm();
    let response = execute(
        &mut vm,
        &source.ok_or("expected -c, script or --worker")?,
        false,
    );
    print!("{}", vm.output);
    if response["ok"] != true {
        eprintln!("{}", response["error"].as_str().unwrap());
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_requests_share_a_session_and_reset_discards_it() {
        for tier in [Tiering::Bytecode, Tiering::Native] {
            let options = Options {
                tier,
                readonly: true,
                mutation: true,
                diagnostics: false,
            };
            let mut vm = options.vm();
            assert_eq!(execute(&mut vm, "data=[1,2,3]", false)["ok"], true);
            assert_eq!(execute(&mut vm, "print(len(data))", false)["stdout"], "3\n");
            vm = options.vm();
            assert_eq!(execute(&mut vm, "print(data)", false)["ok"], false);
            assert_eq!(
                execute(&mut vm, "print('東京\\nquoted \\\"')", false)["stdout"],
                "東京\nquoted \"\n"
            );
        }
    }
}
