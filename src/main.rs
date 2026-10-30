use std::{
    env, fs,
    io::{self, Read},
    process,
};
use tis::{BytecodeMode, NumericMode, Tiering, Vm, compile};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut tier = Tiering::Bytecode;
    let mut bytecode = BytecodeMode::default();
    let mut numeric = NumericMode::default();
    let mut source = None;
    let mut stats = false;
    let mut cfg_inlining = false;
    let mut readonly_regions = false;
    let mut mutation_regions = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("tis {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-c" => {
                if source.is_some() {
                    return Err("specify only one input".into());
                }
                source = Some(args.next().ok_or("-c requires Python code")?);
            }
            "-" => {
                if source.is_some() {
                    return Err("specify only one input".into());
                }
                let mut input = String::new();
                io::stdin().read_to_string(&mut input)?;
                source = Some(input);
            }
            "--" => {
                if source.is_some() {
                    return Err("specify only one input".into());
                }
                source = Some(fs::read_to_string(
                    args.next().ok_or("-- requires a filename")?,
                )?);
                if args.next().is_some() {
                    return Err("script arguments are not supported".into());
                }
                break;
            }
            "--numeric" => {
                numeric = match args.next().as_deref() {
                    Some("adaptive") => NumericMode::Adaptive,
                    Some("disabled") => NumericMode::Disabled,
                    _ => return Err("--numeric requires adaptive or disabled".into()),
                };
            }
            "--bytecode" => {
                bytecode = match args.next().as_deref() {
                    Some("stack") => BytecodeMode::Stack,
                    Some("register") => BytecodeMode::Register,
                    _ => return Err("--bytecode requires stack or register".into()),
                };
            }
            "--tier" => {
                tier = match args.next().as_deref() {
                    Some("bytecode") => Tiering::Bytecode,
                    Some("lir") => Tiering::Lir,
                    Some("native") => Tiering::Native,
                    Some("copy-patch") => Tiering::CopyPatch,
                    _ => return Err("--tier requires bytecode, lir, native or copy-patch".into()),
                }
            }
            "--stats" => stats = true,
            "--cfg-inlining" => cfg_inlining = true,
            "--readonly-regions" => readonly_regions = true,
            "--mutation-regions" => mutation_regions = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option: {flag}").into()),
            path if source.is_none() => source = Some(fs::read_to_string(path)?),
            _ => return Err("unexpected argument".into()),
        }
    }
    let source = source.ok_or("expected FILE.py, -c CODE or - (see --help)")?;
    let mut program = compile(&source)?;
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.cfg_inlining = cfg_inlining;
    vm.readonly_regions = readonly_regions;
    vm.mutation_regions = mutation_regions;
    vm.bytecode = bytecode;
    vm.numeric = numeric;
    let result = vm.run_program(&mut program);
    print!("{}", vm.output);
    if stats {
        eprintln!("{:?}", vm.tier_stats);
    }
    result?;
    Ok(())
}

const HELP: &str = "Usage: tis [OPTIONS] FILE.py
       tis [OPTIONS] -c CODE
       tis [OPTIONS] -

Run Python code from a file, inline code, or standard input.

Options:
  --tier bytecode|lir|native|copy-patch  Execution tier (default: bytecode)
  --bytecode stack|register            Interpreter representation (default: register)
  --numeric adaptive|disabled         Numeric specialization (default: adaptive)
  --stats                             Print tier statistics to stderr
  --cfg-inlining                      Enable experimental CFG inlining
  --readonly-regions                  Enable experimental read-only regions
  --mutation-regions                  Enable experimental mutation regions
  --                                  Treat the next argument as a filename
  -h, --help                          Show help
  -V, --version                       Show version";
