/// oberih test — вбудований тест-ранер.
/// Знаходить всі fn що починаються з "test" і запускає їх.
/// Зелений = повернув 0 або true, червоний = будь-яка помилка або не 0.

use crate::parser::ast::{Program, Item};
use crate::compiler::Compiler;
use crate::vm::{VM, Value, RuntimeError};

#[derive(Debug)]
pub struct TestResult {
    pub name:    String,
    pub passed:  bool,
    pub message: Option<String>,
    pub ms:      u128,
}

pub fn run_tests(program: &Program) -> Vec<TestResult> {
    // Знаходимо всі fn test*
    let test_fns: Vec<String> = program.items.iter()
        .filter_map(|item| {
            if let Item::Fn(f) = item {
                if f.name.starts_with("test") && f.params.is_empty() {
                    return Some(f.name.clone());
                }
            }
            None
        })
        .collect();

    if test_fns.is_empty() {
        return vec![];
    }

    // Компілюємо один раз
    let module = match Compiler::new().compile_program(program) {
        Ok(m)  => m,
        Err(e) => {
            return vec![TestResult {
                name:    "compile".into(),
                passed:  false,
                message: Some(e.to_string()),
                ms:      0,
            }];
        }
    };

    let mut results = Vec::new();

    for name in &test_fns {
        let start = std::time::Instant::now();
        let mut vm = VM::new(module.clone());

        let (passed, message) = match vm.call_fn(name, vec![]) {
            Ok(Value::Bool(true))  => (true,  None),
            Ok(Value::Nil)         => (true,  None),  // void test — pass
            Ok(Value::Num(n)) if n == 0.0 => (true, None),
            Ok(Value::Bool(false)) => (false, Some("повернув false".into())),
            Ok(other) => (false, Some(format!("повернув {}", other))),
            Err(crate::vm::RuntimeError::General(msg)) => (false, Some(msg)),
            Err(crate::vm::RuntimeError::PropagateErr(v)) => {
                (false, Some(format!("незахоплений Err({})", v)))
            }
            Err(e) => (false, Some(e.to_string())),
        };

        let ms = start.elapsed().as_millis();
        results.push(TestResult { name: name.clone(), passed, message, ms });
    }

    results
}

pub fn print_test_results(results: &[TestResult]) {
    if results.is_empty() {
        println!("Тестів не знайдено. Функції тестів мають починатись з 'test'.");
        println!("Приклад: fn testAdd() -> Bool {{ return add(1, 2) == 3 }}");
        return;
    }

    let total  = results.len();
    let passed = results.iter().filter(|r| r.passed).count();
    let failed = total - passed;

    println!("\nОberih Test Runner");
    println!("{}", "─".repeat(40));

    for r in results {
        if r.passed {
            println!("  \x1b[32m✓\x1b[0m {} \x1b[90m({}ms)\x1b[0m", r.name, r.ms);
        } else {
            println!("  \x1b[31m✗\x1b[0m {} \x1b[90m({}ms)\x1b[0m", r.name, r.ms);
            if let Some(msg) = &r.message {
                println!("      \x1b[31m{}\x1b[0m", msg);
            }
        }
    }

    println!("{}", "─".repeat(40));

    if failed == 0 {
        println!("\x1b[32m{}/{} тестів пройшли\x1b[0m", passed, total);
    } else {
        println!(
            "\x1b[32m{} пройшли\x1b[0m, \x1b[31m{} провалились\x1b[0m (всього {})",
            passed, failed, total
        );
        std::process::exit(1);
    }
}
