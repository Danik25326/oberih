/// Oberih REPL — інтерактивний режим виконання.

use std::io::{self, Write};
use crate::lexer::Lexer;
use crate::parser::{Parser, ast::*};
use crate::compiler::Compiler;
use crate::vm::{VM, Value};

pub fn run_repl() {
    println!();
    println!("  ██████╗ ██████╗ ███████╗██████╗ ██╗██╗  ██╗");
    println!(" ██╔═══██╗██╔══██╗██╔════╝██╔══██╗██║██║  ██║");
    println!(" ██║   ██║██████╔╝█████╗  ██████╔╝██║███████║");
    println!(" ██║   ██║██╔══██╗██╔══╝  ██╔══██╗██║██╔══██║");
    println!(" ╚██████╔╝██████╔╝███████╗██║  ██║██║██║  ██║");
    println!("  ╚═════╝ ╚═════╝ ╚══════╝╚═╝  ╚═╝╚═╝╚═╝  ╚═╝");
    println!();
    println!("  Oberih REPL v0.3.0 — власний рантайм");
    println!("  :help — довідка  |  :quit — вийти  |  :fns — функції");
    println!();

    let mut fn_decls: Vec<Item> = Vec::new();

    loop {
        print!(">>> ");
        io::stdout().flush().ok();

        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => { println!("\nДо побачення!"); break; }
            Ok(_) => {}
        }

        let input = line.trim().to_string();
        if input.is_empty() { continue; }

        match input.as_str() {
            ":quit" | ":q" => { println!("До побачення!"); break; }
            ":help" | ":h" => { print_help(); continue; }
            ":clear"       => { fn_decls.clear(); println!("Очищено."); continue; }
            ":fns"         => {
                if fn_decls.is_empty() {
                    println!("(порожньо)");
                } else {
                    for item in &fn_decls {
                        match item {
                            Item::Fn(f)     => println!("  fn {}", f.name),
                            Item::Struct(s) => println!("  struct {}", s.name),
                            Item::Enum(e)   => println!("  enum {}", e.name),
                            _ => {}
                        }
                    }
                }
                continue;
            }
            _ => {}
        }

        let is_decl = input.starts_with("fn ")
            || input.starts_with("resilient ")
            || input.starts_with("struct ")
            || input.starts_with("enum ")
            || input.starts_with("private ");

        if is_decl {
            let tokens = match Lexer::new(&input).tokenize() {
                Ok(t)  => t,
                Err(e) => { eprintln!("Лексична помилка: {}", e); continue; }
            };
            let program = match Parser::new(tokens).parse_program() {
                Ok(p)  => p,
                Err(e) => { eprintln!("Синтаксична помилка: {}", e); continue; }
            };
            for item in program.items {
                match &item {
                    Item::Fn(f)     => println!("+ fn {}", f.name),
                    Item::Struct(s) => println!("+ struct {}", s.name),
                    Item::Enum(e)   => println!("+ enum {}", e.name),
                    _ => {}
                }
                fn_decls.push(item);
            }
            continue;
        }

        // Вираз або let
        let var_name = if input.starts_with("let ") {
            let after_let = &input[4..];
            after_let.split('=').next().map(|s| s.trim().to_string())
        } else {
            None
        };

        let wrapped = match &var_name {
            Some(name) => format!(
                "fn __eval__() -> Nil {{ {} \n return {} }}",
                input, name
            ),
            None => format!("fn __eval__() -> Nil {{ return {} }}", input),
        };

        let tokens = match Lexer::new(&wrapped).tokenize() {
            Ok(t)  => t,
            Err(e) => { eprintln!("Лексична помилка: {}", e); continue; }
        };
        let program = match Parser::new(tokens).parse_program() {
            Ok(p)  => p,
            Err(e) => { eprintln!("Синтаксична помилка: {}", e); continue; }
        };

        let mut all_items = fn_decls.clone();
        all_items.extend(program.items);
        let full_program = Program { items: all_items };

        let module = match Compiler::new().compile_program(&full_program) {
            Ok(m)  => m,
            Err(e) => { eprintln!("Помилка компіляції: {}", e); continue; }
        };

        let mut vm = VM::new(module);
        match vm.call_fn("__eval__", vec![]) {
            Ok(Value::Nil) => {}
            Ok(v)          => println!("= {}", v),
            Err(e)         => eprintln!("Помилка: {}", e),
        }
    }
}

fn print_help() {
    println!();
    println!("Команди:");
    println!("  :help, :h    — ця довідка");
    println!("  :quit, :q    — вийти");
    println!("  :fns         — показати оголошення");
    println!("  :clear       — очистити сесію");
    println!();
    println!("Приклади:");
    println!("  >>> 2 + 3");
    println!("  = 5");
    println!("  >>> let x = 42");
    println!("  = 42");
    println!("  >>> fn square(n: Number) -> Number {{ return n * n }}");
    println!("  + fn square");
    println!("  >>> square(8)");
    println!("  = 64");
    println!();
}
