/// Рушій регулярних виразів Oberih (без залежностей).
///
/// Реалізація — Pike VM: пошук виконується за час O(довжина_тексту × розмір_шаблону)
/// незалежно від шаблону, тому катастрофічний backtracking (ReDoS) неможливий.
/// Семантика — «зліва-найпріоритетніший» (як Perl/JS/Rust): жадібні й ліниві
/// квантифікатори працюють очікувано.
///
/// Підтримується:
///   літерали, `.` (будь-який символ, крім \n), класи `[a-z0-9_]`, `[^...]`,
///   `\d \D \w \W \s \S` (\d — ASCII-цифри; \w — Юнікод-літери/цифри/`_`, тож
///   працює з кирилицею), `\b \B`, `\n \t \r`, екранування `\. \\ \( ...`,
///   якорі `^ $`, групи `( )` (захоплюючі) і `(?: )`, альтернація `|`,
///   квантифікатори `* + ? {n} {n,} {n,m}` та їх ліниві форми (`*?`, `+?`, `??`, `{n,m}?`),
///   прапорець `(?i)` (без урахування регістру) на початку шаблону.
/// Не підтримується: зворотні посилання (`\1`), lookahead/lookbehind, іменовані групи.
///
/// Усі позиції — в СИМВОЛАХ (не в байтах).

const MAX_PATTERN_LEN: usize = 4000;
const MAX_PROGRAM: usize = 50_000;
const MAX_REPEAT: u32 = 1000;
const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone)]
enum ClassItem {
    Range(char, char),
    Digit(bool), // true = заперечення (\D)
    Word(bool),
    Space(bool),
}

#[derive(Debug, Clone)]
struct Class {
    items:   Vec<ClassItem>,
    negated: bool,
}

fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' }

fn lower(c: char) -> char { c.to_lowercase().next().unwrap_or(c) }
fn upper(c: char) -> char { c.to_uppercase().next().unwrap_or(c) }

impl Class {
    fn matches(&self, c: char, ci: bool) -> bool {
        let hit = |c: char| self.items.iter().any(|it| match it {
            ClassItem::Range(a, b) => *a <= c && c <= *b,
            ClassItem::Digit(neg)  => c.is_ascii_digit() != *neg,
            ClassItem::Word(neg)   => is_word(c) != *neg,
            ClassItem::Space(neg)  => c.is_whitespace() != *neg,
        });
        let mut m = hit(c);
        if !m && ci { m = hit(lower(c)) || hit(upper(c)); }
        m != self.negated
    }
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(Class),
    Bol,
    Eol,
    WordB,
    NotWordB,
    Group(Box<Node>, Option<usize>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: Option<u32>, greedy: bool },
}

#[derive(Debug, Clone)]
enum Inst {
    Char(char),
    Any,
    Class(Box<Class>),
    Split(usize, usize), // спершу перша гілка (вища пріоритетність)
    Jmp(usize),
    Save(usize),
    Bol,
    Eol,
    WordB,
    NotWordB,
    Match,
}

// ---------------------------------------------------------------------------
// Парсер
// ---------------------------------------------------------------------------

struct Parser {
    chars:   Vec<char>,
    pos:     usize,
    ngroups: usize,
}

type PR<T> = Result<T, String>;

impl Parser {
    fn peek(&self) -> Option<char> { self.chars.get(self.pos).copied() }
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) { self.pos += 1; true } else { false }
    }
    fn err<T>(&self, msg: &str) -> PR<T> {
        Err(format!("{} (позиція {})", msg, self.pos))
    }

    fn parse_alt(&mut self, depth: usize) -> PR<Node> {
        if depth > MAX_DEPTH { return self.err("надто глибоке вкладення груп"); }
        let mut alts = vec![self.parse_concat(depth)?];
        while self.eat('|') {
            alts.push(self.parse_concat(depth)?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn parse_concat(&mut self, depth: usize) -> PR<Node> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' { break; }
            items.push(self.parse_repeat(depth)?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    fn parse_repeat(&mut self, depth: usize) -> PR<Node> {
        let atom = self.parse_atom(depth)?;
        let (min, max) = match self.peek() {
            Some('*') => { self.pos += 1; (0, None) }
            Some('+') => { self.pos += 1; (1, None) }
            Some('?') => { self.pos += 1; (0, Some(1)) }
            Some('{') => match self.try_parse_braces()? {
                Some(mm) => mm,
                None     => return Ok(atom), // `{` без коректного {n,m} — звичайний символ (розбереться наступним атомом)
            },
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        if matches!(self.peek(), Some('*') | Some('+') | Some('?')) {
            return self.err("вкладені квантифікатори не підтримуються");
        }
        Ok(Node::Repeat { node: Box::new(atom), min, max, greedy })
    }

    /// `{n}`, `{n,}`, `{n,m}`. Повертає None (нічого не споживши), якщо це не квантифікатор.
    fn try_parse_braces(&mut self) -> PR<Option<(u32, Option<u32>)>> {
        let save = self.pos;
        self.pos += 1; // `{`
        let num = |p: &mut Parser| -> Option<u32> {
            let start = p.pos;
            while p.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) { p.pos += 1; }
            if p.pos == start { return None; }
            p.chars[start..p.pos].iter().collect::<String>().parse::<u32>().ok()
        };
        let min = match num(self) { Some(n) => n, None => { self.pos = save; return Ok(None); } };
        let max = if self.eat(',') {
            if self.peek() == Some('}') { None } else {
                match num(self) { Some(n) => Some(n), None => { self.pos = save; return Ok(None); } }
            }
        } else { Some(min) };
        if !self.eat('}') { self.pos = save; return Ok(None); }
        if min > MAX_REPEAT || max.map(|m| m > MAX_REPEAT).unwrap_or(false) {
            return self.err(&format!("кількість повторень не може перевищувати {}", MAX_REPEAT));
        }
        if let Some(m) = max { if m < min { return self.err("у {n,m} має бути n <= m"); } }
        Ok(Some((min, max)))
    }

    fn parse_atom(&mut self, depth: usize) -> PR<Node> {
        let c = self.peek().unwrap();
        self.pos += 1;
        match c {
            '(' => {
                let cap = if self.eat('?') {
                    if self.eat(':') { None } else {
                        return self.err("непідтримувана конструкція групи (є лише `(?:`; `(?i)` — на початку шаблону)");
                    }
                } else {
                    self.ngroups += 1;
                    Some(self.ngroups)
                };
                let inner = self.parse_alt(depth + 1)?;
                if !self.eat(')') { return self.err("незакрита дужка `(`"); }
                Ok(Node::Group(Box::new(inner), cap))
            }
            '[' => self.parse_class(),
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Bol),
            '$' => Ok(Node::Eol),
            '\\' => self.parse_escape(),
            '*' | '+' | '?' => { self.pos -= 1; self.err("квантифікатор без операнда") }
            other => Ok(Node::Char(other)),
        }
    }

    fn parse_escape(&mut self) -> PR<Node> {
        let c = match self.peek() { Some(c) => c, None => return self.err("`\\` в кінці шаблону") };
        self.pos += 1;
        Ok(match c {
            'd' => Node::Class(Class { items: vec![ClassItem::Digit(false)], negated: false }),
            'D' => Node::Class(Class { items: vec![ClassItem::Digit(true)],  negated: false }),
            'w' => Node::Class(Class { items: vec![ClassItem::Word(false)],  negated: false }),
            'W' => Node::Class(Class { items: vec![ClassItem::Word(true)],   negated: false }),
            's' => Node::Class(Class { items: vec![ClassItem::Space(false)], negated: false }),
            'S' => Node::Class(Class { items: vec![ClassItem::Space(true)],  negated: false }),
            'b' => Node::WordB,
            'B' => Node::NotWordB,
            'n' => Node::Char('\n'),
            't' => Node::Char('\t'),
            'r' => Node::Char('\r'),
            c if !c.is_alphanumeric() => Node::Char(c),
            c => { self.pos -= 1; return self.err(&format!("непідтримувана escape-послідовність `\\{}`", c)); }
        })
    }

    fn parse_class(&mut self) -> PR<Node> {
        let negated = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = match self.peek() {
                Some(c) => c,
                None => return self.err("незакритий клас `[`"),
            };
            self.pos += 1;
            if c == ']' && !first { break; }
            first = false;
            let lo = if c == '\\' {
                let e = match self.peek() { Some(e) => e, None => return self.err("`\\` в кінці шаблону") };
                self.pos += 1;
                match e {
                    'd' => { items.push(ClassItem::Digit(false)); continue; }
                    'D' => { items.push(ClassItem::Digit(true));  continue; }
                    'w' => { items.push(ClassItem::Word(false));  continue; }
                    'W' => { items.push(ClassItem::Word(true));   continue; }
                    's' => { items.push(ClassItem::Space(false)); continue; }
                    'S' => { items.push(ClassItem::Space(true));  continue; }
                    'n' => '\n', 't' => '\t', 'r' => '\r',
                    e if !e.is_alphanumeric() => e,
                    e => { self.pos -= 1; return self.err(&format!("непідтримувана escape-послідовність `\\{}`", e)); }
                }
            } else { c };
            // діапазон lo-hi
            if self.peek() == Some('-') && self.chars.get(self.pos + 1).map(|&n| n != ']').unwrap_or(false) {
                self.pos += 1; // `-`
                let mut hi = self.peek().unwrap();
                self.pos += 1;
                if hi == '\\' {
                    let e = match self.peek() { Some(e) => e, None => return self.err("`\\` в кінці шаблону") };
                    self.pos += 1;
                    hi = match e { 'n' => '\n', 't' => '\t', 'r' => '\r', e if !e.is_alphanumeric() => e,
                        _ => return self.err("клас не може бути кінцем діапазону") };
                }
                if hi < lo { return self.err("некоректний діапазон у класі (початок > кінця)"); }
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Range(lo, lo));
            }
        }
        Ok(Node::Class(Class { items, negated }))
    }
}

// ---------------------------------------------------------------------------
// Компілятор
// ---------------------------------------------------------------------------

fn emit(node: &Node, prog: &mut Vec<Inst>) -> PR<()> {
    if prog.len() > MAX_PROGRAM {
        return Err(format!("шаблон завеликий (понад {} інструкцій після розгортання повторень)", MAX_PROGRAM));
    }
    match node {
        Node::Empty    => {}
        Node::Char(c)  => prog.push(Inst::Char(*c)),
        Node::Any      => prog.push(Inst::Any),
        Node::Class(c) => prog.push(Inst::Class(Box::new(c.clone()))),
        Node::Bol      => prog.push(Inst::Bol),
        Node::Eol      => prog.push(Inst::Eol),
        Node::WordB    => prog.push(Inst::WordB),
        Node::NotWordB => prog.push(Inst::NotWordB),
        Node::Group(inner, cap) => match cap {
            Some(k) => {
                prog.push(Inst::Save(2 * k));
                emit(inner, prog)?;
                prog.push(Inst::Save(2 * k + 1));
            }
            None => emit(inner, prog)?,
        },
        Node::Concat(items) => for n in items { emit(n, prog)?; },
        Node::Alt(alts) => {
            let mut jumps = Vec::new();
            for (i, a) in alts.iter().enumerate() {
                if i + 1 < alts.len() {
                    let split = prog.len();
                    prog.push(Inst::Split(0, 0));
                    let l1 = prog.len();
                    emit(a, prog)?;
                    jumps.push(prog.len());
                    prog.push(Inst::Jmp(0));
                    let l2 = prog.len();
                    prog[split] = Inst::Split(l1, l2);
                } else {
                    emit(a, prog)?;
                }
            }
            let end = prog.len();
            for j in jumps { prog[j] = Inst::Jmp(end); }
        }
        Node::Repeat { node, min, max, greedy } => {
            for _ in 0..*min { emit(node, prog)?; }
            match max {
                None => {
                    let split = prog.len();
                    prog.push(Inst::Split(0, 0));
                    let body = prog.len();
                    emit(node, prog)?;
                    prog.push(Inst::Jmp(split));
                    let end = prog.len();
                    prog[split] = if *greedy { Inst::Split(body, end) } else { Inst::Split(end, body) };
                }
                Some(m) => {
                    let mut splits = Vec::new();
                    for _ in *min..*m {
                        splits.push(prog.len());
                        prog.push(Inst::Split(0, 0));
                        emit(node, prog)?;
                    }
                    let end = prog.len();
                    for s in splits {
                        prog[s] = if *greedy { Inst::Split(s + 1, end) } else { Inst::Split(end, s + 1) };
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Regex
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct Regex {
    prog:    Vec<Inst>,
    ngroups: usize,
    ci:      bool,
}

/// Знайдений збіг: діапазони [початок, кінець) у символах; індекс 0 — весь збіг,
/// далі — захоплюючі групи (None — група не брала участі).
pub type Captures = Vec<Option<(usize, usize)>>;

struct Thread {
    pc:   usize,
    caps: Vec<Option<usize>>,
}

impl Regex {
    pub fn new(pattern: &str) -> Result<Regex, String> {
        if pattern.chars().count() > MAX_PATTERN_LEN {
            return Err(format!("шаблон задовгий (максимум {} символів)", MAX_PATTERN_LEN));
        }
        let (ci, body) = match pattern.strip_prefix("(?i)") {
            Some(rest) => (true, rest),
            None       => (false, pattern),
        };
        let mut p = Parser { chars: body.chars().collect(), pos: 0, ngroups: 0 };
        let ast = p.parse_alt(0)?;
        if p.pos < p.chars.len() {
            return p.err("зайва закриваюча дужка `)`");
        }
        let mut prog = vec![Inst::Save(0)];
        emit(&ast, &mut prog)?;
        prog.push(Inst::Save(1));
        prog.push(Inst::Match);
        Ok(Regex { prog, ngroups: p.ngroups, ci })
    }

    pub fn group_count(&self) -> usize { self.ngroups }

    fn char_eq(&self, a: char, b: char) -> bool {
        a == b || (self.ci && lower(a) == lower(b))
    }

    /// Додає потік (розкриваючи Jmp/Split/Save/якорі) у порядку пріоритету.
    /// Ітеративно, щоб довгі ланцюжки інструкцій не переповнювали стек.
    fn add_thread(&self, list: &mut Vec<Thread>, pc0: usize, caps0: Vec<Option<usize>>,
                  text: &[char], pos: usize, visited: &mut [usize], gen: usize) {
        let mut stack = vec![(pc0, caps0)];
        while let Some((mut pc, mut caps)) = stack.pop() {
            loop {
                if visited[pc] == gen { break; }
                visited[pc] = gen;
                match &self.prog[pc] {
                    Inst::Jmp(t) => pc = *t,
                    Inst::Split(a, b) => {
                        stack.push((*b, caps.clone()));
                        pc = *a;
                    }
                    Inst::Save(k) => { caps[*k] = Some(pos); pc += 1; }
                    Inst::Bol => { if pos == 0 { pc += 1 } else { break } }
                    Inst::Eol => { if pos == text.len() { pc += 1 } else { break } }
                    Inst::WordB | Inst::NotWordB => {
                        let before = pos > 0 && is_word(text[pos - 1]);
                        let after  = pos < text.len() && is_word(text[pos]);
                        let at_boundary = before != after;
                        let want = matches!(self.prog[pc], Inst::WordB);
                        if at_boundary == want { pc += 1 } else { break }
                    }
                    Inst::Char(_) | Inst::Any | Inst::Class(_) | Inst::Match => {
                        list.push(Thread { pc, caps });
                        break;
                    }
                }
            }
        }
    }

    /// Найлівіший збіг, що починається не раніше `start` (символьний індекс).
    pub fn find_at(&self, text: &[char], start: usize) -> Option<Captures> {
        if start > text.len() { return None; }
        let nslots = 2 * (self.ngroups + 1);
        let mut visited = vec![0usize; self.prog.len()];
        let mut clist: Vec<Thread> = Vec::new();
        let mut nlist: Vec<Thread> = Vec::new();
        let mut matched: Option<Vec<Option<usize>>> = None;
        let mut pos = start;

        loop {
            if matched.is_none() {
                self.add_thread(&mut clist, 0, vec![None; nslots], text, pos, &mut visited, pos + 1);
            }
            if clist.is_empty() && matched.is_some() { break; }

            for th in clist.drain(..) {
                let advance = match &self.prog[th.pc] {
                    Inst::Char(c)  => pos < text.len() && self.char_eq(text[pos], *c),
                    Inst::Any      => pos < text.len() && text[pos] != '\n',
                    Inst::Class(c) => pos < text.len() && c.matches(text[pos], self.ci),
                    Inst::Match => {
                        matched = Some(th.caps);
                        break; // потоки нижчого пріоритету відкидаємо
                    }
                    _ => false,
                };
                if advance {
                    self.add_thread(&mut nlist, th.pc + 1, th.caps, text, pos + 1, &mut visited, pos + 2);
                }
            }
            std::mem::swap(&mut clist, &mut nlist);
            nlist.clear();

            if pos >= text.len() { break; }
            pos += 1;
        }

        matched.map(|caps| {
            (0..=self.ngroups).map(|g| match (caps[2 * g], caps[2 * g + 1]) {
                (Some(s), Some(e)) => Some((s, e)),
                _ => None,
            }).collect()
        })
    }

    /// Усі неперекриваючі збіги. Порожній збіг просуває пошук на 1 символ.
    pub fn find_all(&self, text: &[char]) -> Vec<Captures> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos <= text.len() {
            match self.find_at(text, pos) {
                None => break,
                Some(caps) => {
                    let (s, e) = caps[0].unwrap();
                    out.push(caps);
                    pos = if e == s { e + 1 } else { e };
                }
            }
        }
        out
    }

    pub fn is_match(&self, text: &[char]) -> bool { self.find_at(text, 0).is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> { s.chars().collect() }
    fn find(p: &str, t: &str) -> Option<String> {
        let re = Regex::new(p).unwrap();
        let tc = chars(t);
        re.find_at(&tc, 0).map(|c| { let (s, e) = c[0].unwrap(); tc[s..e].iter().collect() })
    }
    fn groups(p: &str, t: &str) -> Vec<Option<String>> {
        let re = Regex::new(p).unwrap();
        let tc = chars(t);
        re.find_at(&tc, 0).unwrap().iter()
            .map(|g| g.map(|(s, e)| tc[s..e].iter().collect())).collect()
    }

    #[test]
    fn basics_and_anchors() {
        assert_eq!(find("abc", "xxabcxx"), Some("abc".into()));
        assert_eq!(find("^abc", "xabc"), None);
        assert_eq!(find("abc$", "abcx"), None);
        assert_eq!(find("^abc$", "abc"), Some("abc".into()));
        assert_eq!(find("a.c", "a\nc"), None);
        assert_eq!(find("a.c", "abc"), Some("abc".into()));
        assert_eq!(find("", "abc"), Some("".into()));
    }

    #[test]
    fn quantifiers_greedy_and_lazy() {
        assert_eq!(find("a+", "caaat"), Some("aaa".into()));
        assert_eq!(find("a+?", "caaat"), Some("a".into()));
        assert_eq!(find("<.*>", "<a><b>"), Some("<a><b>".into()));
        assert_eq!(find("<.*?>", "<a><b>"), Some("<a>".into()));
        assert_eq!(find("a{2}", "aaaa"), Some("aa".into()));
        assert_eq!(find("a{2,3}", "aaaa"), Some("aaa".into()));
        assert_eq!(find("a{2,}", "aaaa"), Some("aaaa".into()));
        assert_eq!(find("a{2,3}?", "aaaa"), Some("aa".into()));
        assert_eq!(find("colou?r", "color"), Some("color".into()));
        assert_eq!(find("x*", "aaa"), Some("".into()));
        assert_eq!(find("a{,3}", "a{,3}"), Some("a{,3}".into())); // не квантифікатор — літерал
    }

    #[test]
    fn classes_and_escapes() {
        assert_eq!(find("[a-c]+", "xxabcabx"), Some("abcab".into()));
        assert_eq!(find("[^a-c]+", "abxyzc"), Some("xyz".into()));
        assert_eq!(find(r"\d+", "tel 555-1234"), Some("555".into()));
        assert_eq!(find(r"\D+", "12ab34"), Some("ab".into()));
        assert_eq!(find(r"\s+", "a \t b"), Some(" \t ".into()));
        assert_eq!(find(r"\w+", "  привіт_світ! "), Some("привіт_світ".into()));
        assert_eq!(find(r"[\d.]+", "v3.14x"), Some("3.14".into()));
        assert_eq!(find(r"[]a]+", "]a]"), Some("]a]".into()));
        assert_eq!(find(r"[a\-z]+", "a-z"), Some("a-z".into()));
        assert_eq!(find(r"a\.b", "a.b axb"), Some("a.b".into()));
        assert_eq!(find(r"\(x\)", "(x)"), Some("(x)".into()));
        assert_eq!(find(r"[a-]+", "a-a"), Some("a-a".into()));
    }

    #[test]
    fn groups_and_alternation() {
        assert_eq!(groups(r"(\d+)-(\d+)", "tel 555-1234"),
            vec![Some("555-1234".into()), Some("555".into()), Some("1234".into())]);
        assert_eq!(groups("(a)|(b)", "b"), vec![Some("b".into()), None, Some("b".into())]);
        assert_eq!(find("cat|dog|bird", "hotdog"), Some("dog".into()));
        assert_eq!(find("(?:ab)+", "ababab!"), Some("ababab".into()));
        assert_eq!(groups("(?:a)(b)", "ab"), vec![Some("ab".into()), Some("b".into())]);
        // пріоритет альтернатив (зліва направо), а не найдовший збіг
        assert_eq!(find("a|ab", "ab"), Some("a".into()));
        assert_eq!(find("ab|a", "ab"), Some("ab".into()));
        // остання ітерація групи в повторенні
        assert_eq!(groups("(a|b)+", "abab")[1], Some("b".into()));
    }

    #[test]
    fn word_boundaries_and_case_insensitive() {
        assert_eq!(find(r"\bcat\b", "concat cat!"), Some("cat".into()));
        assert_eq!(find(r"\Bcat", "concat"), Some("cat".into()));
        assert_eq!(find(r"\bкіт\b", "кіт і кітка"), Some("кіт".into()));
        assert_eq!(find("(?i)hello", "Say HeLLo"), Some("HeLLo".into()));
        assert_eq!(find("(?i)[a-z]+", "ABC"), Some("ABC".into()));
        assert_eq!(find("(?i)привіт", "ПРИВІТ"), Some("ПРИВІТ".into()));
        assert_eq!(find("(?i)[^a]", "A"), None);
        assert_eq!(find("hello", "HELLO"), None);
    }

    #[test]
    fn find_all_handles_empty_matches() {
        let re = Regex::new("a*").unwrap();
        let t = chars("baac");
        let m: Vec<(usize, usize)> = re.find_all(&t).iter().map(|c| c[0].unwrap()).collect();
        assert_eq!(m, vec![(0, 0), (1, 3), (3, 3), (4, 4)]);
        let re = Regex::new(r"\d+").unwrap();
        let t = chars("a1b22c333");
        assert_eq!(re.find_all(&t).len(), 3);
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        for bad in ["(", ")", "a)", "[abc", "*a", "+", "a**", "a{2,1}", r"\", r"\1", "(?=a)", "[z-a]",
                    "a{1001}", &"(".repeat(300)] {
            assert!(Regex::new(bad).is_err(), "мало бути помилкою: {:?}", bad);
        }
        // завеликий після розгортання повторень
        assert!(Regex::new("((a{100}){100}){100}").is_err());
    }

    #[test]
    fn no_catastrophic_backtracking() {
        // Класичні ReDoS-шаблони: у backtracking-рушіях це експонента, тут — лінійно.
        let text: String = "a".repeat(5000) + "!";
        let start = std::time::Instant::now();
        for p in ["(a+)+$", "(a|aa)+$", "(a*)*b", "^(a+)+$", "(x+x+)+y"] {
            let re = Regex::new(p).unwrap();
            let _ = re.is_match(&chars(&text));
        }
        assert!(start.elapsed().as_secs_f64() < 5.0, "надто повільно: {:?}", start.elapsed());
    }

    #[test]
    fn long_inputs_and_deep_programs_do_not_overflow_stack() {
        let re = Regex::new("a{1000}").unwrap();
        assert!(re.is_match(&chars(&"a".repeat(1000))));
        assert!(!re.is_match(&chars(&"a".repeat(999))));
        let re = Regex::new(r"(?:\w+\s?)+$").unwrap();
        assert!(re.is_match(&chars(&"слово ".repeat(2000))));
    }
}
