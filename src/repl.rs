/// Oberih REPL — інтерактивний режим виконання.
///
/// Сесія зберігає стан між рядками: оголошення (fn/struct/enum) в `fn_decls`,
/// а значення змінних (`let`) — в `session_vars`. Кожен новий рядок
/// компілюється як функція `__eval__`, що приймає всі відомі сесійні змінні
/// як параметри (їхні реальні Value передаються напряму через call_fn,
/// без перетворення назад у вихідний код), виконує введений рядок і
/// повертає список актуальних значень усіх відстежуваних змінних —
/// так VM-стан коректно "перетікає" з одного виклику в наступний.

use std::io::{self, Write};
use crate::lexer::{Lexer, Token};
use crate::parser::{Parser, ast::*};
use crate::compiler::Compiler;
use crate::vm::{VM, Value};

// ---------------------------------------------------------------------------
// Історія команд
// ---------------------------------------------------------------------------
//
// Без термінала в "сирому" режимі (raw mode) немає способу перехопити стрілки
// вгору/вниз без нової залежності (rustyline/crossterm) — а проєкт свідомо
// тримається нуля зовнішніх залежностей поза TLS. Тому історія працює як у
// найпростіших POSIX-шеллах без readline: `:history` показує список,
// `!N` і `!!` повторюють конкретний або останній запис. Історія також
// зберігається між сеансами у файлі.
//
// Багаторядкові рядки в файлі історії зберігаються в один рядок: реальні
// переведення рядка замінюються на літерал `\n` (і назад при завантаженні).

fn history_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok()?;
    Some(std::path::Path::new(&home).join(".oberih_history"))
}

fn load_history() -> Vec<String> {
    let Some(path) = history_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    text.lines().map(|l| l.replace("\\n", "\n")).collect()
}

fn append_history(entry: &str) {
    let Some(path) = history_path() else { return };
    let line = entry.replace('\n', "\\n");
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", line);
    }
}

/// Читає один рядок з stdin. `None` означає кінець вводу (Ctrl+D / закритий канал).
fn read_line() -> Option<String> {
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
    }
}

/// Верхньорівневе оголошення (`fn`, `struct`, `enum`, з модифікаторами
/// `private`/`resilient`) — це `Item`, а не `Stmt`.
fn is_decl(input: &str) -> bool {
    input.starts_with("fn ")
        || input.starts_with("resilient ")
        || input.starts_with("struct ")
        || input.starts_with("enum ")
        || input.starts_with("private ")
}

/// Чи обірваний ввід посеред незакритої дужки/фігурної/квадратної дужки —
/// рахуємо баланс на рівні ТОКЕНІВ, а не через пробний парс. Пробний парс
/// (обгортання в тіло функції) на перший погляд простіше, але ненадійне:
/// одинична завершена інструкція теж "не парситься до кінця" без явно
/// доданої обгортки, а додана обгортка своєю чергою маскує СПРАВЖНІй Eof
/// всередині незакритого виразу як помилку "неочікуваний токен `}`".
/// Підрахунок дужок такої проблеми не має і автоматично коректний для
/// рядків і коментарів — лексер вже перетворив їх на суцільні токени.
fn looks_incomplete(src: &str) -> bool {
    let tokens = match Lexer::new(src).tokenize() {
        Ok(t)  => t,
        Err(_) => return false, // лексична помилка (напр. незакритий рядок) — показуємо одразу, не чекаємо
    };
    let mut depth: i32 = 0;
    for t in &tokens {
        match t.kind {
            Token::LParen | Token::LBrace | Token::LBracket => depth += 1,
            Token::RParen | Token::RBrace | Token::RBracket => depth -= 1,
            _ => {}
        }
    }
    depth > 0
}

/// Читає один "логічний" ввід REPL: одна команда `:...`, або вираз/інструкція,
/// що може займати кілька рядків (незакриті дужки продовжують читання з `... `).
/// `None` — кінець вводу.
fn read_statement() -> Option<String> {
    print!(">>> ");
    io::stdout().flush().ok();
    let first = read_line()?;
    let trimmed = first.trim();
    if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with('!') {
        return Some(first);
    }

    let mut buf = first;
    // Максимум продовжень — щоб забута лапка чи дужка не перетворила REPL
    // на вічне очікування вводу без жодної діагностики.
    for _ in 0..200 {
        if !looks_incomplete(&buf) { break; }
        print!("... ");
        io::stdout().flush().ok();
        match read_line() {
            Some(next) => { buf.push('\n'); buf.push_str(&next); }
            None => break, // Ctrl+D посеред виразу — виконуємо як є, отримаємо звичайну помилку
        }
    }
    Some(buf)
}

/// Автоматичний прохід збирача циклів після кожного успішного рядка: коренями
/// служить рівно `session_vars` — це ВСІ Value, які сесія ще пам'ятає, тож
/// збирання тут завжди безпечне й точне. Пропускається, якщо в сесії є
/// незавершений `spawn` (див. gc.rs::collect_cycles — чому це важливо).
fn auto_collect_cycles(session_vars: &[(String, Value)]) {
    let has_unjoined_spawn = session_vars.iter().any(|(_, v)| {
        matches!(v, Value::Spawn(h) if !h.done.load(std::sync::atomic::Ordering::Acquire))
    });
    if has_unjoined_spawn { return; }
    let roots: Vec<Value> = session_vars.iter().map(|(_, v)| v.clone()).collect();
    crate::gc::collect_cycles(&roots);
}

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
    println!("  :help — довідка  |  :quit — вийти  |  :fns — функції  |  :vars — змінні  |  :history — історія");
    println!();

    let mut fn_decls: Vec<Item> = Vec::new();
    // Сесійні змінні: (ім'я, поточне значення), у порядку оголошення.
    let mut session_vars: Vec<(String, Value)> = Vec::new();
    // Лямбди, створені в попередніх рядках: значення в session_vars посилаються
    // на них за іменем, тож вони мають існувати в кожному наступному модулі.
    let mut lambda_fns: Vec<crate::compiler::bytecode::CompiledFn> = Vec::new();
    let mut lambda_base: usize = 0;
    let mut history: Vec<String> = load_history();

    loop {
        let raw = match read_statement() {
            Some(s) => s,
            None => { println!("\nДо побачення!"); break; }
        };
        let trimmed_once = raw.trim();
        if trimmed_once.is_empty() { continue; }

        // `!N` — повторити запис №N з `:history`; `!!` — повторити останній.
        let input = if let Some(rest) = trimmed_once.strip_prefix('!') {
            if history.is_empty() {
                eprintln!("Історія порожня.");
                continue;
            }
            let idx = if rest == "!" {
                Some(history.len() - 1)
            } else {
                rest.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&i| i < history.len())
            };
            match idx {
                Some(i) => {
                    let cmd = history[i].clone();
                    println!("{}", cmd);
                    cmd
                }
                None => { eprintln!("Немає запису історії '{}' (див. :history)", trimmed_once); continue; }
            }
        } else {
            raw.trim().to_string()
        };

        if input.is_empty() { continue; }

        // У історію потрапляє все, крім самих команд керування історією —
        // повторювати `:history` чи `!3` через `!N` було б безглуздо.
        if !input.starts_with(':') && !input.starts_with('!') {
            history.push(input.clone());
            append_history(&input);
        }

        match input.as_str() {
            ":quit" | ":q" => { println!("До побачення!"); break; }
            ":help" | ":h" => { print_help(); continue; }
            ":clear"       => {
                fn_decls.clear();
                session_vars.clear();
                println!("Очищено.");
                continue;
            }
            ":vars" => {
                if session_vars.is_empty() {
                    println!("(порожньо)");
                } else {
                    for (name, val) in &session_vars {
                        println!("  {} = {}", name, val);
                    }
                }
                continue;
            }
            ":gc" => {
                let before = crate::gc::gc_stats();
                let has_unjoined_spawn = session_vars.iter().any(|(_, v)| {
                    matches!(v, Value::Spawn(h) if !h.done.load(std::sync::atomic::Ordering::Acquire))
                });
                if has_unjoined_spawn {
                    println!("Пропущено: є незавершений spawn у сесії — розривати цикли зараз небезпечно.");
                } else {
                    let roots: Vec<Value> = session_vars.iter().map(|(_, v)| v.clone()).collect();
                    let swept = crate::gc::collect_cycles(&roots);
                    let after = crate::gc::gc_stats();
                    println!("Розірвано об'єктів у циклах: {}", swept);
                    println!("gc: allocs={} drops={} live={} (було live={})",
                        after.total_allocs, after.total_drops, after.live_objects, before.live_objects);
                }
                continue;
            }
            ":history" => {
                if history.is_empty() {
                    println!("(порожньо)");
                } else {
                    for (i, cmd) in history.iter().enumerate() {
                        let oneline = cmd.replace('\n', "  ⏎ ");
                        println!("  {:>3}  {}", i + 1, oneline);
                    }
                }
                continue;
            }
            ":fns" => {
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

        if is_decl(&input) {
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

        // --- Вираз / let / присвоєння ---
        // Пробний парс рядка як тіла функції — так ми дізнаємось РЕАЛЬНИЙ вид
        // інструкції (Let / Assign / Expr) від самого парсера, а не за
        // рядковими префіксами, як робилось раніше.
        let probe_src = format!("fn __probe__() -> Nil {{ {} }}", input);
        let probe_tokens = match Lexer::new(&probe_src).tokenize() {
            Ok(t)  => t,
            Err(e) => { eprintln!("Лексична помилка: {}", e); continue; }
        };
        let probe_program = match Parser::new(probe_tokens).parse_program() {
            Ok(p)  => p,
            Err(e) => { eprintln!("Синтаксична помилка: {}", e); continue; }
        };
        let probe_stmt = match probe_program.items.into_iter().next() {
            Some(Item::Fn(f)) if f.body.len() == 1 => f.body.into_iter().next().unwrap(),
            Some(Item::Fn(f)) if f.body.is_empty() => {
                // Порожнє тіло (наприклад коментар) — просто ігноруємо.
                let _ = f;
                continue;
            }
            _ => {
                eprintln!("REPL приймає один вираз або одну інструкцію за рядок.");
                continue;
            }
        };

        // Визначаємо display-змінну і чи вводиться НОВА змінна.
        let (body_line, display_name, is_new_var) = match &probe_stmt {
            Stmt::Let { name, .. } => {
                (input.clone(), name.clone(), true)
            }
            Stmt::Assign { target, .. } => {
                if let Expr::Ident(name, _) = target {
                    let known = session_vars.iter().any(|(n, _)| n == name);
                    (input.clone(), name.clone(), !known)
                } else {
                    // x.f = ... або x[i] = ... — немає простого display-імені,
                    // виконуємо як звичайну інструкцію без друку результату.
                    (input.clone(), String::new(), false)
                }
            }
            Stmt::Expr(_) => {
                (format!("let __res__ = ({})", input), "__res__".to_string(), true)
            }
            _ => {
                // If/while/for/return як окремий рядок REPL — виконуємо як є,
                // без display-значення.
                (input.clone(), String::new(), false)
            }
        };

        // Список усіх імен, чиї значення повертаємо з __eval__, у фіксованому
        // порядку: спершу всі вже відомі сесійні змінні, тоді нова (якщо є).
        let mut names: Vec<String> = session_vars.iter().map(|(n, _)| n.clone()).collect();
        if !display_name.is_empty() && !names.contains(&display_name) {
            names.push(display_name.clone());
        }

        let params: String = session_vars
            .iter()
            .map(|(n, _)| format!("{}: Any", n))
            .collect::<Vec<_>>()
            .join(", ");

        let return_expr = if names.is_empty() {
            "[0]".to_string()   // у мові немає літерала nil; порожній список імен не повертаємо
        } else {
            format!("[{}]", names.join(", "))
        };

        let wrapped = format!(
            "fn __eval__({}) -> Nil {{ {}\n return {} }}",
            params, body_line, return_expr
        );

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

        let mut module = match Compiler::new().with_lambda_base(lambda_base).compile_program(&full_program) {
            Ok(m)  => m,
            Err(e) => { eprintln!("Помилка компіляції: {}", e); continue; }
        };
        // Запам'ятовуємо нові лямбди й додаємо раніше створені до цього модуля.
        for f in &module.functions {
            if let Some(n) = f.name.strip_prefix("__lambda_").and_then(|s| s.parse::<usize>().ok()) {
                lambda_base = lambda_base.max(n);
                if !lambda_fns.iter().any(|g| g.name == f.name) { lambda_fns.push(f.clone()); }
            }
        }
        for f in &lambda_fns {
            if module.find_fn(&f.name).is_none() { module.functions.push(f.clone()); }
        }

        let args: Vec<Value> = session_vars.iter().map(|(_, v)| v.clone()).collect();

        // Великий стек — див. vm::run_with_big_stack: інакше глибока рекурсія
        // в рядку REPL кладе ввесь процес, а не повертає звичайну помилку.
        let eval_result = crate::vm::run_with_big_stack(move || {
            let mut vm = VM::new(module);
            vm.call_fn("__eval__", args)
        });
        match eval_result {
            Ok(Value::Nil) => {}
            Ok(Value::List(list)) => {
                let values = list.to_vec();
                // Оновлюємо/додаємо сесійні змінні за новими значеннями.
                for (name, val) in names.iter().zip(values.iter()) {
                    if name == "__res__" { continue; }
                    if let Some(slot) = session_vars.iter_mut().find(|(n, _)| n == name) {
                        slot.1 = val.clone();
                    } else {
                        session_vars.push((name.clone(), val.clone()));
                    }
                }
                // Друкуємо display-значення (крім тихих Nil-виразів типу print()).
                if let Some(pos) = names.iter().position(|n| n == &display_name) {
                    if !display_name.is_empty() {
                        let v = &values[pos];
                        if !matches!(v, Value::Nil) || is_new_var {
                            if !matches!(v, Value::Nil) {
                                println!("= {}", v);
                            }
                        }
                    }
                }
                auto_collect_cycles(&session_vars);
            }
            Ok(v)  => println!("= {}", v),
            Err(e) => eprintln!("Помилка: {}", e),
        }
    }
}

fn print_help() {
    println!();
    println!("Команди:");
    println!("  :help, :h    — ця довідка");
    println!("  :quit, :q    — вийти");
    println!("  :fns         — показати оголошені fn/struct/enum");
    println!("  :vars        — показати сесійні змінні та їхні значення");
    println!("  :history     — показати історію команд цього й попередніх сеансів");
    println!("  :gc          — розірвати цикли (a.next=b; b.prev=a) і показати статистику GC");
    println!("  !N           — повторити команду №N з :history");
    println!("  !!           — повторити останню команду");
    println!("  :clear       — очистити всю сесію (оголошення і змінні)");
    println!();
    println!("Незакриті дужки/фігурні дужки/квадратні дужки продовжують ввід на новому рядку (\"... \").");
    println!();
    println!("Приклади:");
    println!("  >>> 2 + 3");
    println!("  = 5");
    println!("  >>> let x = 42");
    println!("  = 42");
    println!("  >>> x + 1");
    println!("  = 43");
    println!("  >>> fn square(n: Number) -> Number {{ return n * n }}");
    println!("  + fn square");
    println!("  >>> square(x)");
    println!("  = 1764");
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unclosed_brackets() {
        assert!(looks_incomplete("let s = (1 +"));
        assert!(looks_incomplete("if (x > 0) {"));
        assert!(looks_incomplete("let xs = [1, [2, 3"));
        assert!(looks_incomplete("fn square(n: Number) -> Number {"));
        assert!(!looks_incomplete("2 + 3"));
        assert!(!looks_incomplete("let x = (1 + 2)"));
        assert!(!looks_incomplete("print(\"a { b\")")); // дужка в рядку не рахується
        assert!(!looks_incomplete("let x = )")); // зайва закриваюча — не "чекаємо ще", а помилка
        assert!(!looks_incomplete("// { незакритий коментар не рахується"));
    }

    #[test]
    fn recognizes_declarations() {
        assert!(is_decl("fn f() -> Number { return 1 }"));
        assert!(is_decl("struct P { x: Number }"));
        assert!(is_decl("enum E { A, B }"));
        assert!(is_decl("private fn f() -> Number { return 1 }"));
        assert!(is_decl("resilient fn f() -> Number { return 1 }"));
        assert!(!is_decl("let x = 5"));
        assert!(!is_decl("2 + 2"));
    }
}
