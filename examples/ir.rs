mod support;

use std::{env, fs, process};

use tis::{Object, Value, Vm};

const HELP: &str = "Usage: cargo run --example ir -- [OPTIONS]\n\n  --demo add|sum                Function to inspect (default: sum)\n  --phase cold|warm|fallback    Feedback to collect (default: warm)\n  --source PATH                Inspect Python source (always cold)\n  --function NAME              Function in source (default: <module>)\n  --no-promote                 Inspect slot HIR before SSA promotion\n  --format text|dot|html|bytecode|lir|profile        Output format (default: text)\n  --output PATH                Write to a file instead of stdout\n  --help                       Show this help\n\nExample:\n  cargo run --example ir -- --format html --output /tmp/tis-ir.html";

fn main() {
    if let Err(error) = run() {
        eprintln!("tis-ir: {error}");
        process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut demo = "sum".to_owned();
    let mut phase = "warm".to_owned();
    let mut format = "text".to_owned();
    let mut output = None;
    let mut source = None;
    let mut name = "<module>".to_owned();
    let mut promote_locals = true;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--help" {
            println!("{HELP}");
            return Ok(());
        }
        if argument == "--no-promote" {
            promote_locals = false;
            continue;
        }
        let target = match argument.as_str() {
            "--source" => {
                source = Some(arguments.next().ok_or("--source requires a path")?);
                continue;
            }
            "--function" => &mut name,
            "--demo" => &mut demo,
            "--phase" => &mut phase,
            "--format" => &mut format,
            "--output" => {
                output = Some(arguments.next().ok_or("--output requires a path")?);
                continue;
            }
            _ => return Err(format!("unknown option {argument:?}\n{HELP}")),
        };
        *target = arguments
            .next()
            .ok_or_else(|| format!("{argument} requires a value"))?;
    }
    if !["cold", "warm", "fallback"].contains(&phase.as_str()) {
        return Err(format!("unknown phase {phase:?}"));
    }
    let mut function = if let Some(path) = source {
        phase = "cold".into();
        let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
        tis::compile_with_options(&source, tis::CompileOptions { promote_locals })
            .map_err(|error| error.to_string())?
            .function(&name)
            .ok_or_else(|| format!("unknown function {name:?}"))?
            .clone()
    } else {
        match demo.as_str() {
            "add" => support::addition(),
            "sum" => support::sum(),
            _ => return Err(format!("unknown demo {demo:?}")),
        }
    };
    let mut vm = Vm::new();
    vm.profiling = tis::profile::ProfileConfig::eager();
    if phase != "cold" {
        for _ in 0..16 {
            let arguments = if demo == "add" {
                vec![Value::Int(1), Value::Int(2)]
            } else {
                vec![Value::Int(10)]
            };
            vm.run(&mut function, &arguments)
                .map_err(|error| error.to_string())?;
        }
    }
    if phase == "fallback" {
        let arguments = if demo == "add" {
            vec![
                vm.heap.allocate(Object::String("hello ".into())),
                vm.heap.allocate(Object::String("tis".into())),
            ]
        } else {
            vec![Value::Float(10.5)]
        };
        vm.run(&mut function, &arguments)
            .map_err(|error| error.to_string())?;
    }
    let content = match format.as_str() {
        "bytecode" => function
            .bytecode()
            .map_err(|error| error.to_string())?
            .disassemble()
            .map_err(|error| error.to_string())?,
        "lir" => function
            .lower_lir(tis::lir::LoweringMode::Profiled)
            .map_err(|error| error.to_string())?
            .dump(),
        "profile" => function.profile().to_string(),
        "text" => function.dump(),
        "dot" => function.to_dot(),
        "html" => function.to_html(),
        _ => return Err(format!("unknown format {format:?}")),
    };
    if let Some(path) = output {
        fs::write(&path, content).map_err(|error| format!("cannot write {path:?}: {error}"))?;
    } else {
        print!("{content}");
    }
    Ok(())
}
