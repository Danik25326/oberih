/// oberih explain — статичний аналіз Shared Budget.
/// Будує дерево розподілу deadline/retry між функціями
/// і виводить його в термінал.

use std::collections::HashMap;
use crate::parser::ast::*;

// ---------------------------------------------------------------------------
// Вузол дерева бюджету
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct BudgetNode {
    pub fn_name:      String,
    pub deadline:     Option<f64>,   // секунди
    pub retry_budget: Option<u32>,
    pub timeout:      Option<f64>,
    pub is_resilient: bool,
    pub callees:      Vec<String>,   // функції які викликаються з тіла
}

// ---------------------------------------------------------------------------
// Побудова дерева
// ---------------------------------------------------------------------------

pub struct Explainer {
    nodes: HashMap<String, BudgetNode>,
}

impl Explainer {
    pub fn new() -> Self {
        Explainer { nodes: HashMap::new() }
    }

    pub fn build(mut self, program: &Program) -> HashMap<String, BudgetNode> {
        for item in &program.items {
            if let Item::Fn(f) = item {
                let mut node = BudgetNode {
                    fn_name:      f.name.clone(),
                    deadline:     None,
                    retry_budget: None,
                    timeout:      None,
                    is_resilient: f.is_resilient,
                    callees:      Vec::new(),
                };
                for m in &f.modifiers {
                    match m {
                        Modifier::Deadline(d)    => node.deadline     = Some(d.to_secs()),
                        Modifier::RetryBudget(n) => node.retry_budget = Some(*n),
                        Modifier::Timeout(d)     => node.timeout      = Some(d.to_secs()),
                        _ => {}
                    }
                }
                collect_callees(&f.body, &mut node.callees);
                self.nodes.insert(f.name.clone(), node);
            }
        }
        self.nodes
    }
}

fn collect_callees(block: &Block, out: &mut Vec<String>) {
    for stmt in block {
        collect_callees_stmt(stmt, out);
    }
}

fn collect_callees_stmt(stmt: &Stmt, out: &mut Vec<String>) {
    match stmt {
        Stmt::Let    { value, .. }  => collect_callees_expr(value, out),
        Stmt::Return { value, .. }  => collect_callees_expr(value, out),
        Stmt::Assign { value, .. }  => collect_callees_expr(value, out),
        Stmt::Expr(e)               => collect_callees_expr(e, out),
        Stmt::If   { cond, then_body, else_body, .. } => {
            collect_callees_expr(cond, out);
            collect_callees(then_body, out);
            if let Some(b) = else_body { collect_callees(b, out); }
        }
        Stmt::While { cond, body, .. } => {
            collect_callees_expr(cond, out);
            collect_callees(body, out);
        }
        Stmt::For { iter, body, .. } => {
            collect_callees_expr(iter, out);
            collect_callees(body, out);
        }
    }
}

fn collect_callees_expr(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Call { callee, args, .. } => {
            if let Expr::Ident(name, _) = callee.as_ref() {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            for a in args { collect_callees_expr(a, out); }
        }
        Expr::MethodCall { object, args, .. } => {
            collect_callees_expr(object, out);
            for a in args { collect_callees_expr(a, out); }
        }
        Expr::BinOp { left, right, .. } => {
            collect_callees_expr(left, out);
            collect_callees_expr(right, out);
        }
        Expr::Try  { expr, .. } => collect_callees_expr(expr, out),
        Expr::Neg  { expr, .. } => collect_callees_expr(expr, out),
        Expr::Field { object, .. } => collect_callees_expr(object, out),
        Expr::Index { object, index, .. } => {
            collect_callees_expr(object, out);
            collect_callees_expr(index, out);
        }
        Expr::Match { scrutinee, arms, .. } => {
            collect_callees_expr(scrutinee, out);
            for arm in arms { collect_callees_expr(&arm.body, out); }
        }
        Expr::Spawn { fn_name, args, .. } => {
            if !out.contains(fn_name) { out.push(fn_name.clone()); }
            for a in args { collect_callees_expr(a, out); }
        }
        Expr::List(elems, _) => {
            for e in elems { collect_callees_expr(e, out); }
        }
        Expr::Map(entries, _) => {
            for (k, v) in entries {
                collect_callees_expr(k, out);
                collect_callees_expr(v, out);
            }
        }
        Expr::ResultCtor { value, .. } => collect_callees_expr(value, out),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Рендер дерева в термінал
// ---------------------------------------------------------------------------

pub fn print_budget_tree(
    nodes:    &HashMap<String, BudgetNode>,
    root:     &str,
    indent:   usize,
    visited:  &mut Vec<String>,
) {
    let prefix = "  ".repeat(indent);
    let node = match nodes.get(root) {
        Some(n) => n,
        None    => {
            println!("{}└─ {} (зовнішня / вбудована)", prefix, root);
            return;
        }
    };

    // Іконка
    let icon = if node.is_resilient { "⚡" } else { "·" };

    // Deadline / timeout рядок
    let mut budget_parts: Vec<String> = Vec::new();
    if let Some(d) = node.deadline {
        budget_parts.push(format!("deadline={:.0}ms", d * 1000.0));
    }
    if let Some(t) = node.timeout {
        budget_parts.push(format!("timeout={:.0}ms", t * 1000.0));
    }
    if let Some(r) = node.retry_budget {
        budget_parts.push(format!("retries={}", r));
    }

    let budget_str = if budget_parts.is_empty() {
        String::new()
    } else {
        format!("  [{}]", budget_parts.join(", "))
    };

    println!("{}{} {}{}", prefix, icon, root, budget_str);

    // Рекурсивно для callees
    if visited.contains(&root.to_string()) {
        println!("{}  (цикл — пропускаємо)", prefix);
        return;
    }
    visited.push(root.to_string());

    for callee in &node.callees {
        // Пропускаємо вбудовані
        if matches!(
            callee.as_str(),
            "println" | "print" | "len" | "toString" | "toNumber"
            | "readFile" | "writeFile" | "now" | "exit"
        ) {
            continue;
        }
        print_budget_tree(nodes, callee, indent + 1, visited);
    }

    visited.pop();
}

/// Форматована таблиця worst-case оцінок.
pub fn print_forecast(nodes: &HashMap<String, BudgetNode>, root: &str) {
    println!("\n── Forecast (worst-case) ──");
    let total = forecast_secs(nodes, root, &mut Vec::new());
    println!("  {} → max {:.0}ms", root, total * 1000.0);
}

fn forecast_secs(
    nodes:   &HashMap<String, BudgetNode>,
    name:    &str,
    visited: &mut Vec<String>,
) -> f64 {
    if visited.contains(&name.to_string()) { return 0.0; }
    let node = match nodes.get(name) {
        Some(n) => n,
        None    => return 0.0,
    };

    // Якщо є deadline — це верхня межа
    if let Some(d) = node.deadline {
        let retries = node.retry_budget.unwrap_or(1) as f64;
        return d * retries;
    }

    // Інакше — сума callees
    visited.push(name.to_string());
    let sum: f64 = node.callees.iter()
        .map(|c| forecast_secs(nodes, c, visited))
        .sum();
    visited.pop();

    if let Some(t) = node.timeout { t.min(sum) } else { sum }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn build_nodes(src: &str) -> HashMap<String, BudgetNode> {
        let tokens  = Lexer::new(src).tokenize().unwrap();
        let program = Parser::new(tokens).parse_program().unwrap();
        Explainer::new().build(&program)
    }

    #[test]
    fn test_deadline_captured() {
        let src = r#"
resilient fn getOrder(id: String) -> String
    deadline(2s)
    retryBudget(3)
{
    return fetchOrder(id)
}
fn fetchOrder(id: String) -> String { return id }
"#;
        let nodes = build_nodes(src);
        let node = nodes.get("getOrder").unwrap();
        assert_eq!(node.deadline, Some(2.0));
        assert_eq!(node.retry_budget, Some(3));
        assert!(node.callees.contains(&"fetchOrder".to_string()));
    }

    #[test]
    fn test_callee_collection() {
        let src = r#"
fn main() -> Number {
    let a = foo(1)
    let b = bar(2)
    return 0
}
fn foo(x: Number) -> Number { return x }
fn bar(x: Number) -> Number { return x }
"#;
        let nodes = build_nodes(src);
        let node = nodes.get("main").unwrap();
        assert!(node.callees.contains(&"foo".to_string()));
        assert!(node.callees.contains(&"bar".to_string()));
    }

    #[test]
    fn test_forecast() {
        let src = r#"
resilient fn root(x: String) -> String
    deadline(5s)
    retryBudget(2)
{
    return child(x)
}
fn child(x: String) -> String { return x }
"#;
        let nodes = build_nodes(src);
        // forecast = deadline * retries = 5 * 2 = 10s
        let total = forecast_secs(&nodes, "root", &mut Vec::new());
        assert!((total - 10.0).abs() < 0.01);
    }
}
