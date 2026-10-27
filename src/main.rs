mod lexer;
mod parser;
mod compiler;
mod vm;
mod gc;
mod json;
mod http_client;
mod module_loader;
mod repl;
mod typechecker;
mod explain;
mod stdlib;
mod stdlib_ext;
mod regex;
mod regex_ops;
mod diagnostics;
mod formatter;
mod test_runner;
mod manifest;
mod deps;
mod docs_data;

use std::fs;
use std::path::Path;
use rustls;

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
    // Ініціалізуємо rustls crypto provider (ring) для HTTPS
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok(); // ok() — ігноруємо якщо вже встановлено

    enable_ansi_on_windows();
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_help();
        std::process::exit(1);
    }

    match args[1].as_str() {
        "run"      => cmd_run(&args),
        "check"    => cmd_check(&args),
        "repl"     => repl::run_repl(),
        "fmt"      => cmd_fmt(&args),
        "test"     => cmd_test(&args),
        "explain"  => cmd_explain(&args),
        "init"     => cmd_init(&args),
        "add"      => cmd_add(&args),
        "docs"     => cmd_docs(&args),
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
    println!("  oberih <команда>                  — у директорії з oberih.toml файл не потрібен");
    println!();
    println!("КОМАНДИ:");
    println!("  run      [файл]          Виконати програму");
    println!("  repl                     Інтерактивний режим");
    println!("  check    [файл]          Перевірити синтаксис і типи");
    println!("  fmt      <файл>          Форматувати код");
    println!("  test     [файл]          Запустити тести (fn test*)");
    println!("  explain  [файл] [fn]     Показати дерево Shared Budget");
    println!("  tokens   <файл>          Показати токени");
    println!("  ast      <файл>          Показати AST");
    println!("  bytecode <файл>          Показати bytecode");
    println!("  init     [ім'я]          Створити новий проєкт (oberih.toml + src/main.obh)");
    println!("  add      <ім'я> ...      Додати залежність до oberih.toml");
    println!("  docs     [файл.md]       Згенерувати довідку по stdlib (stdout або файл)");
    println!("  version                  Версія");
    println!("  gc-stats                 Статистика garbage collector");
    println!();
    println!("ПАКЕТИ (oberih.toml):");
    println!("  oberih init my-app                          Новий проєкт у ./my-app");
    println!("  oberih add mathutils --path ../mathutils    Залежність за локальним шляхом");
    println!("  oberih add web --git <url> --branch main    Залежність з git-репозиторію");
    println!("  import \"mathutils/utils.obh\"                Імпорт з залежності за її ім'ям");
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
    let path = resolve_target_path(args.get(2));
    let (src, program) = load_and_parse(&path);

    // Typechecker — попередження при run, помилки тільки при check
    if let Err(errs) = typechecker::Typechecker::new().check(&program) {
        // Фільтруємо — зупиняємо тільки на критичних помилках
        // Помилки поля на Unknown типі — це обмеження typechecker, не реальна помилка
        let critical: Vec<_> = errs.iter()
            .filter(|e| !e.message.contains("не-struct") || !e.message.contains("Number"))
            .collect();
        if !critical.is_empty() {
            let diags: Vec<diagnostics::Diagnostic> = critical.iter()
                .map(|e| diagnostics::Diagnostic::error(&e.message, e.line, e.col).with_source(&src))
                .collect();
            diagnostics::print_diagnostics(&diags);
            std::process::exit(1);
        }
    }

    // Compile
    let module = match compiler::Compiler::new().compile_program(&program) {
        Ok(m)  => m,
        Err(e) => {
            eprintln!("{}", diagnostics::explain_runtime_error(&e.to_string(), 0, 0));
            std::process::exit(1);
        }
    };

    // Run. На окремому потоці з великим стеком — див. коментар при
    // vm::run_with_big_stack: типового стека головного потоку вистачає лише
    // на ~2500 рівнів рекурсії Oberih-функцій, що зависоко для практики.
    let (result, call_stack_str) = vm::run_with_big_stack(move || {
        let mut vm = vm::VM::new(module);
        let result = vm.run();
        let stack = vm.format_call_stack();
        (result, stack)
    });
    match result {
        Ok(vm::Value::Nil) | Ok(vm::Value::Num(_)) => {}
        Ok(v) => println!("{}", v),
        Err(vm::RuntimeError::General(msg)) => {
            let d = diagnostics::explain_runtime_error(&msg, 0, 0);
            eprintln!("{}", d);
            if !call_stack_str.is_empty() {
                eprintln!("{}", call_stack_str);
            }
            std::process::exit(1);
        }
        Err(vm::RuntimeError::PropagateErr(v)) => {
            let d = diagnostics::explain_runtime_error(
                &format!("незахоплена помилка: Err({})", v), 0, 0
            );
            eprintln!("{}", d);
            if !call_stack_str.is_empty() {
                eprintln!("{}", call_stack_str);
            }
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Помилка: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_check(args: &[String]) {
    let path = resolve_target_path(args.get(2));
    let (src, program) = load_and_parse(&path);

    match typechecker::Typechecker::new().check(&program) {
        Ok(_) => println!("\x1b[32m✓\x1b[0m {} — синтаксис і типи коректні", path),
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
    let src = read_source(&args[2]);

    let (tokens, comments) = match lexer::Lexer::new(&src).tokenize_with_comments() {
        Ok(t)  => t,
        Err(e) => {
            eprintln!("{}", diagnostics::Diagnostic::error(&e.message, e.line, e.col).with_source(&src));
            std::process::exit(1);
        }
    };
    let program = match parser::Parser::new(tokens).parse_program() {
        Ok(p)  => p,
        Err(e) => {
            eprintln!("{}", diagnostics::Diagnostic::error(&e.message, e.line, e.col).with_source(&src));
            std::process::exit(1);
        }
    };

    let formatted = formatter::Formatter::new().format_program_with_comments(&program, comments);

    // Записуємо назад
    std::fs::write(&args[2], &formatted)
        .unwrap_or_else(|e| { eprintln!("Не вдалось записати: {}", e); std::process::exit(1); });
    println!("\x1b[32m✓\x1b[0m {} відформатовано", args[2]);
}

fn cmd_test(args: &[String]) {
    let path = resolve_target_path(args.get(2));
    let (_, program) = load_and_parse(&path);
    let results = test_runner::run_tests(&program);
    test_runner::print_test_results(&results);
}

fn cmd_docs(args: &[String]) {
    // `oberih docs` -> друкує в stdout; `oberih docs файл.md` -> записує у файл.
    let mut out = String::new();
    out.push_str("# Довідка по стандартній бібліотеці Oberih\n\n");
    out.push_str(&format!("Згенеровано: `oberih docs` (версія {}).\n\n", env!("CARGO_PKG_VERSION")));
    out.push_str("## Зміст\n\n");
    for section in docs_data::SECTIONS {
        out.push_str(&format!("- [{}](#{})\n", section.title, slugify(section.title)));
    }
    out.push('\n');
    for section in docs_data::SECTIONS {
        out.push_str(&format!("## {}\n\n", section.title));
        for entry in section.entries {
            out.push_str(&format!("### `{}`\n\n", entry.sig));
            out.push_str(&format!("{}\n\n", entry.doc));
            out.push_str(&format!("```\n{}\n```\n\n", entry.example));
        }
    }

    match args.get(2) {
        Some(path) => {
            fs::write(path, &out).unwrap_or_else(|e| { eprintln!("Не вдалось записати {}: {}", path, e); std::process::exit(1); });
            println!("Записано: {}", path);
        }
        None => print!("{}", out),
    }
}

/// Заголовок секції -> якір markdown-посилання (як це роблять GitHub/більшість рендерерів).
fn slugify(title: &str) -> String {
    title.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn cmd_init(args: &[String]) {
    // `oberih init` — у поточній директорії; `oberih init <ім'я>` — створює нову.
    let (dir, name) = match args.get(2) {
        Some(n) => (Path::new(n).to_path_buf(), n.clone()),
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
            let name = cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "app".to_string());
            (cwd, name)
        }
    };

    if dir.join(manifest::MANIFEST_FILE).exists() {
        eprintln!("{} вже існує в {}", manifest::MANIFEST_FILE, dir.display());
        std::process::exit(1);
    }
    fs::create_dir_all(dir.join("src")).unwrap_or_else(|e| {
        eprintln!("Не вдалось створити {}: {}", dir.join("src").display(), e);
        std::process::exit(1);
    });

    let man = manifest::Manifest {
        name: name.clone(), version: "0.1.0".to_string(), entry: "src/main.obh".to_string(),
        dependencies: Vec::new(), root: dir.clone(),
    };
    man.save(&dir.join(manifest::MANIFEST_FILE)).unwrap_or_else(|e| {
        eprintln!("Не вдалось записати {}: {}", manifest::MANIFEST_FILE, e);
        std::process::exit(1);
    });

    let main_obh = dir.join("src/main.obh");
    if !main_obh.exists() {
        fs::write(&main_obh, format!(
            "// {}: точка входу проєкту.\n\nfn main() -> Number {{\n    println(\"Привіт від {}!\")\n    return 0\n}}\n",
            name, name
        )).unwrap_or_else(|e| { eprintln!("Не вдалось записати {}: {}", main_obh.display(), e); std::process::exit(1); });
    }

    println!("Створено проєкт '{}' у {}", name, dir.display());
    println!("  {}", manifest::MANIFEST_FILE);
    println!("  src/main.obh");
    println!("\nЗапуск: cd {} && oberih run", dir.display());
}

fn cmd_add(args: &[String]) {
    if args.len() < 3 {
        eprintln!("Використання:");
        eprintln!("  oberih add <ім'я> --path <шлях>");
        eprintln!("  oberih add <ім'я> --git <url> [--branch <гілка> | --tag <тег> | --rev <ревізія>]");
        std::process::exit(1);
    }
    let dep_name = args[2].clone();

    let mut path_val: Option<String> = None;
    let mut git_val:  Option<String> = None;
    let mut branch: Option<String> = None;
    let mut tag:    Option<String> = None;
    let mut rev:    Option<String> = None;
    let mut i = 3;
    while i < args.len() {
        let val = |i: usize| -> String {
            args.get(i).cloned().unwrap_or_else(|| {
                eprintln!("{}: бракує значення", args[i - 1]);
                std::process::exit(1);
            })
        };
        match args[i].as_str() {
            "--path"   => { path_val = Some(val(i + 1)); i += 2; }
            "--git"    => { git_val  = Some(val(i + 1)); i += 2; }
            "--branch" => { branch   = Some(val(i + 1)); i += 2; }
            "--tag"    => { tag      = Some(val(i + 1)); i += 2; }
            "--rev"    => { rev      = Some(val(i + 1)); i += 2; }
            other => { eprintln!("Невідомий прапорець: {}", other); std::process::exit(1); }
        }
    }

    let source = match (path_val, git_val) {
        (Some(p), None) => manifest::DepSource::Path(p),
        (None, Some(url)) => manifest::DepSource::Git { url, branch, tag, rev },
        (Some(_), Some(_)) => { eprintln!("Вкажіть або --path, або --git, не обидва"); std::process::exit(1); }
        (None, None) => { eprintln!("Потрібен --path <шлях> або --git <url>"); std::process::exit(1); }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let manifest_path = manifest::find_manifest(&cwd).unwrap_or_else(|| {
        eprintln!("{} не знайдено (спочатку `oberih init`)", manifest::MANIFEST_FILE);
        std::process::exit(1);
    });
    let man = manifest::load(&manifest_path).unwrap_or_else(|e| { eprintln!("{}", e); std::process::exit(1); });
    let man = man.with_dependency(manifest::Dependency { name: dep_name.clone(), source });

    // Перевіряємо одразу (path — існування; git — реальне клонування), щоб не
    // записувати в маніфест залежність, яку неможливо розв'язати.
    if let Err(e) = deps::resolve_all(&man) {
        eprintln!("Не вдалось додати '{}': {}", dep_name, e);
        std::process::exit(1);
    }
    man.save(&manifest_path).unwrap_or_else(|e| { eprintln!("Не вдалось записати {}: {}", manifest_path.display(), e); std::process::exit(1); });
    println!("Додано залежність '{}' до {}", dep_name, manifest_path.display());
}

fn cmd_explain(args: &[String]) {
    // Без аргументів узагалі — використовуємо точку входу з oberih.toml, корінь "main".
    // З аргументом файлу — як і раніше: `oberih explain <файл.obh> [fn]`.
    let path = resolve_target_path(args.get(2));
    let (_, program) = load_and_parse(&path);
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

/// Визначає, який файл запускати: явно вказаний аргументом, або — якщо
/// аргумента немає — точку входу з `oberih.toml` (шукається вгору від
/// поточної директорії, як `Cargo.toml`). Без жодного з двох — помилка з
/// підказкою.
fn resolve_target_path(explicit: Option<&String>) -> String {
    if let Some(p) = explicit { return p.clone(); }
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    match manifest::find_manifest(&cwd) {
        Some(manifest_path) => match manifest::load(&manifest_path) {
            Ok(man) => man.root.join(&man.entry).to_string_lossy().into_owned(),
            Err(e) => { eprintln!("{}", e); std::process::exit(1); }
        },
        None => {
            eprintln!("Потрібен файл: вкажіть <файл.obh> або запустіть у директорії з oberih.toml (див. `oberih init`)");
            std::process::exit(1);
        }
    }
}

/// Якщо для `path` знайдено `oberih.toml` (у його директорії чи вище),
/// розв'язує всі залежності маніфесту в локальні директорії. Немає
/// маніфесту — порожня карта, і поведінка ідентична попереднім версіям
/// (окремий файл без проєкту).
fn resolve_packages_for(path: &str) -> std::collections::HashMap<String, std::path::PathBuf> {
    let start = Path::new(path).parent().unwrap_or_else(|| Path::new("."));
    match manifest::find_manifest(start) {
        None => std::collections::HashMap::new(),
        Some(manifest_path) => match manifest::load(&manifest_path) {
            Ok(man) => match deps::resolve_all(&man) {
                Ok(pkgs) => pkgs,
                Err(e) => { eprintln!("Помилка залежностей: {}", e); std::process::exit(1); }
            },
            Err(e) => { eprintln!("{}", e); std::process::exit(1); }
        }
    }
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

    // Розгортаємо import декларації (з урахуванням залежностей з oberih.toml, якщо є)
    let packages = resolve_packages_for(path);
    let base_path = std::path::Path::new(path);
    let program = match module_loader::resolve_imports_with_packages(program, base_path, &packages) {
        Ok(p)  => p,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    (src, program)
}
