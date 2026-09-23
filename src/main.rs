mod lexer;
mod parser;
mod compiler;
mod vm;
mod gc;
mod typechecker;
mod explain;
mod stdlib;
mod diagnostics;
mod formatter;
mod test_runner;

use std::fs;
use std::path::Path;

fn enable_ansi_on_windows() {
    #[cfg(windows)]
    {
        // SetConsoleMode через std::process — без unsafe
        let _ = std::process::Command::new("cmd")
            .args(["/C", ""])
            .status();
        // Альтернативно — просто встановлюємо змінну середовища
        std::env::set_var("TERM", "xterm-256color");
    }
}

fn main() {
    enable_ansi_on_windows();
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_help();
        std::process::exit(1);
    }

    match args[1].as_str() {
        "run"      => cmd_run(&args),
        "check"    => cmd_check(&args),
        "fmt"      => cmd_fmt(&args),
        "test"     => cmd_test(&args),
        "explain"  => cmd_explain(&args),
        "tokens"   => cmd_tokens(&args),
        "ast"      => cmd_ast(&args),
        "bytecode" => cmd_bytecode(&args),
        "version"  => println!("Oberih 0.3.0 — власний рантайм, reference counting GC"),
        "gc-stats" => {
            let stats = gc::gc_stats();
            println!("GC Statistics:");
            println!("  Всього алокацій:  {}", stats.total_allocs);
            println!("  Звільнено:        {}", stats.total_drops);
            println!("  Живих об'єктів:   {}", stats.live_objects);
        }
        "help" | "--help" | "-h" => print_help(),
        cmd => {
            eprintln!("Невідома команда: '{}'\n", cmd);
            print_help();
            std::process::exit(1);
        }
    }
}

fn print_help() {
    println!("Oberih — мова програмування з вбудованою стійкістю до збоїв");
    println!();
    println!("ВИКОРИСТАННЯ:");
    println!("  oberih <команда> <файл.obh> [аргументи]");
    println!();
    println!("КОМАНДИ:");
    println!("  run      <файл>          Виконати програму");
    println!("  check    <файл>          Перевірити синтаксис і типи");
    println!("  fmt      <файл>          Форматувати код");
    println!("  test     <файл>          Запустити тести (fn test*)");
    println!("  explain  <файл> [fn]     Показати дерево Shared Budget");
    println!("  tokens   <файл>          Показати токени");
    println!("  ast      <файл>          Показати AST");
    println!("  bytecode <файл>          Показати bytecode");
    println!("  version                  Версія");
    println!("  gc-stats                 Статистика garbage collector");
    println!();
    println!("ПРИКЛАДИ:");
    println!("  oberih run examples/cli_tool.obh");
    println!("  oberih test examples/cli_tool.obh");
    println!("  oberih explain examples/cli_tool.obh fetchHealth");
    println!("  oberih fmt examples/cli_tool.obh");
}

// ---------------------------------------------------------------------------
// Команди
// ---------------------------------------------------------------------------

fn cmd_run(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл: oberih run <файл.obh>"); std::process::exit(1); }
    let (src, program) = load_and_parse(&args[2]);

    // Typechecker
    if let Err(errs) = typechecker::Typechecker::new().check(&program) {
        let diags: Vec<diagnostics::Diagnostic> = errs.iter()
            .map(|e| diagnostics::Diagnostic::error(&e.message, e.line, e.col).with_source(&src))
            .collect();
        diagnostics::print_diagnostics(&diags);
        std::process::exit(1);
    }

    // Compile
    let module = match compiler::Compiler::new().compile_program(&program) {
        Ok(m)  => m,
        Err(e) => {
            eprintln!("{}", diagnostics::explain_runtime_error(&e.to_string(), 0, 0));
            std::process::exit(1);
        }
    };

    // Run
    let mut vm = vm::VM::new(module);
    match vm.run() {
        Ok(vm::Value::Nil) | Ok(vm::Value::Num(_)) => {}
        Ok(v) => println!("{}", v),
        Err(vm::RuntimeError::General(msg)) => {
            let d = diagnostics::explain_runtime_error(&msg, 0, 0);
            eprintln!("{}", d);
            std::process::exit(1);
        }
        Err(vm::RuntimeError::PropagateErr(v)) => {
            let d = diagnostics::explain_runtime_error(
                &format!("незахоплена помилка: Err({})", v), 0, 0
            );
            eprintln!("{}", d);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Помилка: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_check(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл: oberih check <файл.obh>"); std::process::exit(1); }
    let (src, program) = load_and_parse(&args[2]);

    match typechecker::Typechecker::new().check(&program) {
        Ok(_) => println!("\x1b[32m✓\x1b[0m {} — синтаксис і типи коректні", args[2]),
        Err(errs) => {
            let diags: Vec<diagnostics::Diagnostic> = errs.iter()
                .map(|e| diagnostics::Diagnostic::error(&e.message, e.line, e.col).with_source(&src))
                .collect();
            diagnostics::print_diagnostics(&diags);
            std::process::exit(1);
        }
    }
}

fn cmd_fmt(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл: oberih fmt <файл.obh>"); std::process::exit(1); }
    let (_, program) = load_and_parse(&args[2]);
    let formatted = formatter::Formatter::new().format_program(&program);

    // Записуємо назад
    std::fs::write(&args[2], &formatted)
        .unwrap_or_else(|e| { eprintln!("Не вдалось записати: {}", e); std::process::exit(1); });
    println!("\x1b[32m✓\x1b[0m {} відформатовано", args[2]);
}

fn cmd_test(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл: oberih test <файл.obh>"); std::process::exit(1); }
    let (_, program) = load_and_parse(&args[2]);
    let results = test_runner::run_tests(&program);
    test_runner::print_test_results(&results);
}

fn cmd_explain(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл: oberih explain <файл.obh> [fn]"); std::process::exit(1); }
    let (_, program) = load_and_parse(&args[2]);
    let nodes = explain::Explainer::new().build(&program);

    let root = args.get(3).cloned().unwrap_or_else(|| "main".to_string());

    println!("\nShared Budget Tree: {}", root);
    println!("{}", "─".repeat(40));
    explain::print_budget_tree(&nodes, &root, 0, &mut Vec::new());
    explain::print_forecast(&nodes, &root);
    println!();
}

fn cmd_tokens(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл"); std::process::exit(1); }
    let src = read_source(&args[2]);
    match lexer::Lexer::new(&src).tokenize() {
        Ok(toks) => {
            for t in &toks {
                println!("{:>4}:{:<3} {:?}", t.span.line, t.span.col, t.kind);
            }
        }
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    }
}

fn cmd_ast(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл"); std::process::exit(1); }
    let (_, program) = load_and_parse(&args[2]);
    println!("{:#?}", program);
}

fn cmd_bytecode(args: &[String]) {
    if args.len() < 3 { eprintln!("Потрібен файл"); std::process::exit(1); }
    let (_, program) = load_and_parse(&args[2]);
    match compiler::Compiler::new().compile_program(&program) {
        Ok(module) => {
            for func in &module.functions {
                println!("=== fn {} ({} локальних) ===", func.name, func.local_count);
                if func.resilience.deadline_secs.is_some() {
                    println!("  resilience: deadline={:?}s retry={:?}",
                        func.resilience.deadline_secs,
                        func.resilience.retry_budget);
                }
                for (i, instr) in func.code.iter().enumerate() {
                    println!("  {:04}  {:?}", i, instr);
                }
                println!();
            }
        }
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    }
}

// ---------------------------------------------------------------------------
// Хелпери
// ---------------------------------------------------------------------------

fn read_source(path: &str) -> String {
    if !Path::new(path).exists() {
        eprintln!("\x1b[31mПомилка:\x1b[0m Файл не знайдено: {}", path);
        std::process::exit(1);
    }
    fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("Не вдалось прочитати {}: {}", path, e);
        std::process::exit(1);
    })
}

fn load_and_parse(path: &str) -> (String, parser::ast::Program) {
    let src = read_source(path);

    let tokens = match lexer::Lexer::new(&src).tokenize() {
        Ok(t)  => t,
        Err(e) => {
            eprintln!("{}", diagnostics::Diagnostic::error(
                &e.message, e.line, e.col
            ).with_source(&src));
            std::process::exit(1);
        }
    };

    let program = match parser::Parser::new(tokens).parse_program() {
        Ok(p)  => p,
        Err(e) => {
            eprintln!("{}", diagnostics::Diagnostic::error(
                &e.message, e.line, e.col
            ).with_source(&src));
            std::process::exit(1);
        }
    };

    (src, program)
}
