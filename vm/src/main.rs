mod lexer;
mod parser;
mod compiler;
mod vm;

use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Oberih VM — власний рантайм мови Oberih");
        eprintln!("");
        eprintln!("Використання:");
        eprintln!("  oberih run <file.obh>        — виконати програму");
        eprintln!("  oberih check <file.obh>      — перевірити синтаксис");
        eprintln!("  oberih tokens <file.obh>     — показати токени");
        eprintln!("  oberih ast <file.obh>        — показати AST");
        eprintln!("  oberih bytecode <file.obh>   — показати bytecode");
        std::process::exit(1);
    }

    let command = &args[1];

    match command.as_str() {
        "run" => {
            if args.len() < 3 {
                eprintln!("Потрібен файл: oberih run <file.obh>");
                std::process::exit(1);
            }
            run_file(&args[2]);
        }
        "check" => {
            if args.len() < 3 {
                eprintln!("Потрібен файл: oberih check <file.obh>");
                std::process::exit(1);
            }
            check_file(&args[2]);
        }
        "tokens" => {
            if args.len() < 3 {
                eprintln!("Потрібен файл: oberih tokens <file.obh>");
                std::process::exit(1);
            }
            show_tokens(&args[2]);
        }
        "ast" => {
            if args.len() < 3 {
                eprintln!("Потрібен файл: oberih ast <file.obh>");
                std::process::exit(1);
            }
            show_ast(&args[2]);
        }
        "bytecode" => {
            if args.len() < 3 {
                eprintln!("Потрібен файл: oberih bytecode <file.obh>");
                std::process::exit(1);
            }
            show_bytecode(&args[2]);
        }
        _ => {
            eprintln!("Невідома команда: {}", command);
            eprintln!("Доступні: run, check, tokens, ast, bytecode");
            std::process::exit(1);
        }
    }
}

fn read_source(path: &str) -> String {
    if !Path::new(path).exists() {
        eprintln!("Файл не знайдено: {}", path);
        std::process::exit(1);
    }
    match fs::read_to_string(path) {
        Ok(s)  => s,
        Err(e) => {
            eprintln!("Не вдалось прочитати {}: {}", path, e);
            std::process::exit(1);
        }
    }
}

fn run_file(path: &str) {
    let src = read_source(path);

    // 1. Лексер
    let tokens = match lexer::Lexer::new(&src).tokenize() {
        Ok(t)  => t,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // 2. Парсер
    let program = match parser::Parser::new(tokens).parse_program() {
        Ok(p)  => p,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // 3. Компілятор
    let module = match compiler::Compiler::new().compile_program(&program) {
        Ok(m)  => m,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    // 4. VM
    let mut vm = vm::VM::new(module);
    match vm.run() {
        Ok(result) => {
            // Виводимо результат тільки якщо він не nil
            match result {
                vm::Value::Nil => {}
                v              => println!("{}", v),
            }
        }
        Err(e) => {
            eprintln!("Помилка виконання: {}", e);
            std::process::exit(1);
        }
    }
}

fn check_file(path: &str) {
    let src = read_source(path);

    let tokens = match lexer::Lexer::new(&src).tokenize() {
        Ok(t)  => t,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };

    match parser::Parser::new(tokens).parse_program() {
        Ok(_)  => println!("OK: {} — синтаксис правильний", path),
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    }
}

fn show_tokens(path: &str) {
    let src = read_source(path);
    match lexer::Lexer::new(&src).tokenize() {
        Ok(tokens) => {
            for tok in &tokens {
                println!("{:3}:{:2}  {:?}", tok.span.line, tok.span.col, tok.kind);
            }
        }
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    }
}

fn show_ast(path: &str) {
    let src = read_source(path);
    let tokens = match lexer::Lexer::new(&src).tokenize() {
        Ok(t)  => t,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };
    match parser::Parser::new(tokens).parse_program() {
        Ok(p)  => println!("{:#?}", p),
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    }
}

fn show_bytecode(path: &str) {
    let src = read_source(path);
    let tokens = match lexer::Lexer::new(&src).tokenize() {
        Ok(t)  => t,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };
    let program = match parser::Parser::new(tokens).parse_program() {
        Ok(p)  => p,
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    };
    match compiler::Compiler::new().compile_program(&program) {
        Ok(module) => {
            for func in &module.functions {
                println!("=== fn {} ({} locals) ===", func.name, func.local_count);
                for (i, instr) in func.code.iter().enumerate() {
                    println!("  {:04}  {:?}", i, instr);
                }
                println!();
            }
        }
        Err(e) => { eprintln!("{}", e); std::process::exit(1); }
    }
}
