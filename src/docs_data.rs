/// Структуровані дані для `oberih docs` — єдине джерело довідки по stdlib.
///
/// Чесне обмеження: цей список веде вручну поряд з фактичними
/// `is_core_builtin`/`stdlib_ext::is_ext`/`regex_ops::is_ext` у трьох інших
/// файлах — немає єдиного реєстру, з якого й диспетчер, і документація
/// читали б той самий список (це вимагало б рефакторингу робочих
/// диспетчерів заради документації, ризиковано для вже працюючого коду).
/// Тест нижче (`all_documented_names_are_real_builtins`) гарантує напрямок
/// "кожне задокументоване ім'я справді існує" — але НЕ гарантує зворотного
/// (що жодна нова вбудована функція не забута тут). Якщо додаєш builtin —
/// додай і сюди.
pub struct Entry {
    pub name:    &'static str,
    pub sig:     &'static str,
    pub doc:     &'static str,
    pub example: &'static str,
}

pub struct Section {
    pub title:   &'static str,
    pub entries: &'static [Entry],
}

macro_rules! e {
    ($name:expr, $sig:expr, $doc:expr, $example:expr) => {
        Entry { name: $name, sig: $sig, doc: $doc, example: $example }
    };
}

pub const SECTIONS: &[Section] = &[
    Section { title: "Ввід/вивід", entries: &[
        e!("print",      "print(...)",            "Друкує аргументи через пробіл, БЕЗ переведення рядка в кінці.", "print(\"x =\", x)"),
        e!("println",    "println(...)",          "Як `print`, але з переведенням рядка в кінці. Без аргументів — просто порожній рядок.", "println(\"готово\")"),
        e!("readLine",   "readLine() -> String",  "Читає один рядок зі стандартного вводу (без символу переведення рядка).", "let name = readLine()"),
        e!("readFile",   "readFile(path: String) -> Result<String, String>", "Читає файл цілком як текст.", "let text = readFile(\"data.txt\")?"),
        e!("writeFile",  "writeFile(path: String, content: String) -> Result<Nil, String>", "Перезаписує файл цілком.", "writeFile(\"out.txt\", \"привіт\")?"),
        e!("appendFile", "appendFile(path: String, content: String) -> Result<Nil, String>", "Дописує в кінець файлу (створює, якщо не існує).", "appendFile(\"log.txt\", \"рядок\\n\")?"),
    ]},
    Section { title: "HTTP", entries: &[
        e!("httpGet",    "httpGet(url: String) -> Result<Map, String>",                 "GET-запит (http:// або https://). Повертає Map з полями `status` і `body`.", "let r = httpGet(\"https://example.com\")?"),
        e!("httpPost",   "httpPost(url: String, body: String) -> Result<Map, String>",   "POST-запит з тілом.", "httpPost(url, jsonStringify(data))?"),
        e!("httpPut",    "httpPut(url: String, body: String) -> Result<Map, String>",    "PUT-запит з тілом.", "httpPut(url, body)?"),
        e!("httpDelete", "httpDelete(url: String) -> Result<Map, String>",               "DELETE-запит.", "httpDelete(url)?"),
    ]},
    Section { title: "JSON (об'єкти ↔ Map)", entries: &[
        e!("jsonParse",     "jsonParse(s: String) -> Result<Value, String>", "Розбирає JSON. Об'єкти стають `Map`, масиви — `List`.", "let cfg = jsonParse(text)?"),
        e!("jsonStringify", "jsonStringify(v: Value) -> String",             "Серіалізує в компактний однорядковий JSON.", "jsonStringify({\"a\": 1})"),
        e!("jsonPretty",    "jsonPretty(v: Value) -> String",                "Як `jsonStringify`, але з відступами для читабельності.", "jsonPretty(cfg)"),
    ]},
    Section { title: "Конвертація типів", entries: &[
        e!("toString", "toString(v: Value) -> String", "Будь-яке значення в рядок (те саме представлення, що й `print`).", "toString(42)"),
        e!("toNumber", "toNumber(s: String) -> Number", "Рядок у число; падає з помилкою виконання, якщо рядок не число (див. `parseNumber` для безпечної версії).", "toNumber(\"3.14\")"),
        e!("toBool",   "toBool(v: Value) -> Bool",      "Приведення до Bool (0, \"\", Nil — хибні; інше — true).", "toBool(x)"),
    ]},
    Section { title: "Рядки", entries: &[
        e!("strLen",        "strLen(s: String) -> Number",                     "Довжина в СИМВОЛАХ (не байтах) — коректно для кирилиці/емодзі.", "strLen(\"привіт\")"),
        e!("strTrim",       "strTrim(s: String) -> String",                    "Прибирає пробіли з обох країв.", "strTrim(\"  x  \")"),
        e!("strUpper",      "strUpper(s: String) -> String",                   "У верхній регістр.", "strUpper(\"abc\")"),
        e!("strLower",      "strLower(s: String) -> String",                   "У нижній регістр.", "strLower(\"ABC\")"),
        e!("strContains",   "strContains(s: String, sub: String) -> Bool",     "Чи містить підрядок.", "strContains(\"hello\", \"ell\")"),
        e!("strStartsWith", "strStartsWith(s: String, p: String) -> Bool",     "Чи починається з префікса.", "strStartsWith(\"hello\", \"he\")"),
        e!("strEndsWith",   "strEndsWith(s: String, p: String) -> Bool",       "Чи закінчується суфіксом.", "strEndsWith(\"hello\", \"lo\")"),
        e!("strSplit",      "strSplit(s: String, sep: String) -> List<String>","Розбиває за роздільником.", "strSplit(\"a,b,c\", \",\")"),
        e!("strJoin",       "strJoin(sep: String, xs: List<String>) -> String","З'єднує список рядків через роздільник (приймає і зворотний порядок аргументів).", "strJoin(\"-\", [\"a\", \"b\"])"),
        e!("strReplace",    "strReplace(s: String, from: String, to: String) -> String", "Замінює ВСІ входження підрядка.", "strReplace(\"aXb\", \"X\", \"_\")"),
        e!("strSlice",      "strSlice(s: String, from: Number, to: Number) -> String",   "Підрядок [from, to) за символами.", "strSlice(\"hello\", 1, 3)"),
        e!("strIndexOf",    "strIndexOf(s: String, sub: String) -> Number",    "Індекс (у символах) першого входження, або -1.", "strIndexOf(\"hello\", \"l\")"),
        e!("strRepeat",     "strRepeat(s: String, n: Number) -> String",       "Повторює рядок n разів.", "strRepeat(\"ab\", 3)"),
        e!("strPadLeft",    "strPadLeft(s: String, width: Number, fill: String?) -> String",  "Доповнює ЗЛІВА до ширини (символ-заповнювач типово — пробіл).", "strPadLeft(\"7\", 3, \"0\")"),
        e!("strPadRight",   "strPadRight(s: String, width: Number, fill: String?) -> String", "Доповнює СПРАВА до ширини.", "strPadRight(\"ab\", 5, \".\")"),
    ]},
    Section { title: "Числа", entries: &[
        e!("floor", "floor(x: Number) -> Number", "Округлення вниз.", "floor(2.7)"),
        e!("ceil",  "ceil(x: Number) -> Number",  "Округлення вгору.", "ceil(2.1)"),
        e!("round", "round(x: Number) -> Number", "Округлення до найближчого цілого.", "round(2.5)"),
        e!("abs",   "abs(x: Number) -> Number",   "Модуль числа.", "abs(0 - 3)"),
        e!("sqrt",  "sqrt(x: Number) -> Number",  "Квадратний корінь.", "sqrt(16)"),
        e!("pow",   "pow(base: Number, exp: Number) -> Number", "Піднесення до степеня.", "pow(2, 10)"),
        e!("min",   "min(a: Number, b: Number) -> Number", "Менше з двох.", "min(3, 1)"),
        e!("max",   "max(a: Number, b: Number) -> Number", "Більше з двох.", "max(3, 1)"),
        e!("sin",   "sin(x: Number) -> Number", "Синус (радіани).", "sin(pi() / 2)"),
        e!("cos",   "cos(x: Number) -> Number", "Косинус (радіани).", "cos(0)"),
        e!("tan",   "tan(x: Number) -> Number", "Тангенс (радіани).", "tan(0)"),
        e!("log",   "log(x: Number, base: Number?) -> Number", "Натуральний логарифм, або за довільною основою з другим аргументом.", "log(1024, 2)"),
        e!("exp",   "exp(x: Number) -> Number", "e в степені x.", "exp(1)"),
        e!("pi",    "pi() -> Number", "Число π.", "pi()"),
        e!("sign",  "sign(x: Number) -> Number", "-1, 0 або 1 залежно від знаку.", "sign(0 - 5)"),
        e!("random",     "random() -> Number", "Псевдовипадкове з [0, 1).", "random()"),
        e!("randInt",    "randInt(lo: Number, hi: Number) -> Number", "Псевдовипадкове ціле з [lo, hi] (межі включно).", "randInt(1, 6)"),
        e!("seedRandom", "seedRandom(n: Number) -> Nil", "Фіксує послідовність `random`/`randInt` (для тестів).", "seedRandom(42)"),
        e!("parseNumber","parseNumber(s: String) -> Result<Number, String>", "Безпечний `toNumber` — повертає Err замість падіння на нечисловому рядку.", "parseNumber(\"abc\")"),
    ]},
    Section { title: "Map", entries: &[
        e!("keys",      "keys(m: Map) -> List",            "Ключі в порядку вставки. Так само: `m.keys()`.", "keys({\"a\": 1})"),
        e!("values",    "values(m: Map) -> List",           "Значення в порядку вставки.", "values(m)"),
        e!("entries",   "entries(m: Map) -> List<List>",    "Пари [ключ, значення].", "entries(m)"),
        e!("mapHas",    "mapHas(m: Map, k) -> Bool",        "Чи є ключ. Так само: `m.has(k)`.", "mapHas(m, \"a\")"),
        e!("mapGet",    "mapGet(m: Map, k, default?) -> Value", "Значення або `default` (типово Nil), якщо ключа немає.", "mapGet(m, \"x\", 0)"),
        e!("mapSet",    "mapSet(m: Map, k, v) -> Map",      "Встановлює значення НА МІСЦІ, повертає той самий Map.", "mapSet(m, \"a\", 1)"),
        e!("mapDelete", "mapDelete(m: Map, k) -> Value",    "Видаляє ключ, повертає видалене значення (або Nil).", "mapDelete(m, \"a\")"),
        e!("mapMerge",  "mapMerge(a: Map, b: Map) -> Map",  "НОВИЙ Map: вміст `a` + `b` (при збігу ключів виграє `b`).", "mapMerge(defaults, overrides)"),
    ]},
    Section { title: "Списки", entries: &[
        e!("len",      "len(x: List|Map|String) -> Number", "Довжина/кількість елементів.", "len([1, 2, 3])"),
        e!("push",     "push(xs: List, v) -> List",    "НОВИЙ список з доданим елементом (xs не змінюється).", "push([1, 2], 3)"),
        e!("append",   "append(xs: List, v) -> List",  "Додає елемент НА МІСЦІ (O(1)), повертає той самий список.", "append(xs, 3)"),
        e!("pop",      "pop(xs: List) -> List",        "Новий список без останнього елемента.", "pop([1, 2, 3])"),
        e!("first",    "first(xs: List) -> Value",     "Перший елемент.", "first([1, 2])"),
        e!("last",     "last(xs: List) -> Value",      "Останній елемент.", "last([1, 2])"),
        e!("reverse",  "reverse(xs: List) -> List",    "Новий список у зворотному порядку.", "reverse([1, 2, 3])"),
        e!("contains", "contains(xs: List, v) -> Bool","Чи є елемент у списку.", "contains([1, 2], 2)"),
        e!("indexOf",  "indexOf(xs: List, v) -> Number","Індекс першого входження або -1.", "indexOf(xs, 4)"),
        e!("range",    "range(from: Number, to: Number) -> List<Number>", "Список [from, to).", "range(0, 5)"),
        e!("slice",    "slice(xs: List, from: Number, to: Number?) -> List", "Підсписок [from, to) (to типово — до кінця).", "slice(xs, 1, 3)"),
        e!("concat",   "concat(a: List, b: List) -> List", "Новий список — об'єднання двох.", "concat([1], [2])"),
        e!("sort",     "sort(xs: List) -> List",       "Новий відсортований список (Number, String або Bool).", "sort([3, 1, 2])"),
        e!("sum",      "sum(xs: List<Number>) -> Number","Сума елементів.", "sum([1, 2, 3.5])"),
    ]},
    Section { title: "Функції вищого порядку (приймають ім'я функції або лямбду)", entries: &[
        e!("map",    "map(xs: List, f: Fn) -> List",        "Новий список: f(x) для кожного x.", "map(xs, fn(x) => x * 2)"),
        e!("filter", "filter(xs: List, f: Fn) -> List",      "Новий список з елементів, де f(x) істинне.", "filter(xs, isEven)"),
        e!("reduce", "reduce(xs: List, f: Fn, init?) -> Value", "Згортає список: f(acc, x). Без init бере перший елемент.", "reduce(xs, add, 0)"),
        e!("each",   "each(xs: List, f: Fn) -> Nil",          "Викликає f(x) для кожного елемента (заради побічного ефекту).", "each(xs, println)"),
        e!("any",    "any(xs: List, f: Fn) -> Bool",          "Чи істинне f(x) хоч для одного елемента.", "any(xs, isNegative)"),
        e!("all",    "all(xs: List, f: Fn) -> Bool",          "Чи істинне f(x) для всіх елементів.", "all(xs, isPositive)"),
        e!("find",   "find(xs: List, f: Fn) -> Value",        "Перший елемент, де f(x) істинне, або Nil.", "find(xs, isEven)"),
        e!("sortBy", "sortBy(xs: List, keyFn: Fn) -> List",   "Стабільне сортування за ключем keyFn(x).", "sortBy(people, fn(p) => p.age)"),
    ]},
    Section { title: "Час", entries: &[
        e!("now",           "now() -> Number",      "Поточний час, мс від 1970-01-01 UTC.", "let t = now()"),
        e!("sleep",         "sleep(ms: Number) -> Nil", "Блокує поточний потік на вказану кількість мілісекунд.", "sleep(1000)"),
        e!("isoTime",       "isoTime(ms: Number, offsetMin: Number?) -> String", "ISO 8601 рядок; необов'язковий зсув від UTC у хвилинах.", "isoTime(now(), 180)"),
        e!("dateParts",     "dateParts(ms: Number, offsetMin: Number?) -> Map", "{year, month, day, hour, minute, second, millisecond, weekday(1=Пн)}.", "dateParts(now()).year"),
        e!("timeFromParts", "timeFromParts(y, mo, d, h?, mi?, s?, offsetMin?) -> Number", "Складові дати/часу -> мс від епохи.", "timeFromParts(2026, 9, 28)"),
        e!("parseIso",      "parseIso(s: String) -> Result<Number, String>", "Розбирає ISO 8601 рядок у мс від епохи.", "parseIso(\"2026-09-28T12:00:00Z\")?"),
    ]},
    Section { title: "Файли (додатково)", entries: &[
        e!("fileExists", "fileExists(path: String) -> Bool",            "Чи існує файл/директорія за шляхом.", "fileExists(\"cfg.toml\")"),
        e!("readLines",  "readLines(path: String) -> Result<List<String>, String>", "Файл як список рядків (без завершального порожнього).", "readLines(\"log.txt\")?"),
        e!("listDir",    "listDir(path: String) -> Result<List<String>, String>",   "Імена файлів у директорії, відсортовані.", "listDir(\".\")?"),
        e!("deleteFile", "deleteFile(path: String) -> Result<Nil, String>",         "Видаляє файл.", "deleteFile(\"tmp.txt\")?"),
        e!("mkdir",      "mkdir(path: String) -> Result<Nil, String>",              "Створює директорію (і проміжні теки).", "mkdir(\"out/sub\")?"),
    ]},
    Section { title: "Регулярні вирази", entries: &[
        e!("regex",         "regex(pattern: String) -> Result<Regex, String>", "Компілює шаблон один раз — ефективно для повторного використання.", "let re = regex(\"\\\\d+\")?"),
        e!("reTest",        "reTest(pattern: String, s: String) -> Bool",      "Чи є збіг. Компілює шаблон щоразу — для одноразового використання.", "reTest(\"(?i)hello\", s)"),
        e!("reFind",        "reFind(pattern: String, s: String) -> Map|Nil",   "Перший збіг: {text, start, end, groups} або Nil.", "reFind(\"\\\\d+\", s)"),
        e!("reFindAll",     "reFindAll(pattern: String, s: String) -> List<Map>", "Усі неперекриваючі збіги.", "reFindAll(\"\\\\w+\", s)"),
        e!("reReplace",     "reReplace(pattern: String, s: String, repl: String) -> String", "Замінює ПЕРШИЙ збіг. `$1`, `$2` — посилання на групи.", "reReplace(\"(\\\\d+)\", s, \"[$1]\")"),
        e!("reReplaceAll",  "reReplaceAll(pattern: String, s: String, repl: String) -> String", "Замінює ВСІ збіги.", "reReplaceAll(\"\\\\d\", s, \"*\")"),
        e!("reSplit",       "reSplit(pattern: String, s: String) -> List<String>", "Розбиває рядок за шаблоном-роздільником.", "reSplit(\"\\\\s*,\\\\s*\", s)"),
    ]},
    Section { title: "Методи Regex (на значенні, отриманому від regex(...))", entries: &[
        e!("Regex.test",       "r.test(s: String) -> Bool",           "Чи є збіг.", "r.test(s)"),
        e!("Regex.find",       "r.find(s: String) -> Map|Nil",        "Перший збіг.", "r.find(s)"),
        e!("Regex.findAll",    "r.findAll(s: String) -> List<Map>",   "Усі збіги.", "r.findAll(s)"),
        e!("Regex.replace",    "r.replace(s, repl: String) -> String","Заміна першого збігу.", "r.replace(s, \"$1\")"),
        e!("Regex.replaceAll", "r.replaceAll(s, repl: String) -> String", "Заміна всіх збігів.", "r.replaceAll(s, \"*\")"),
        e!("Regex.split",      "r.split(s: String) -> List<String>",  "Розбиття за шаблоном.", "r.split(s)"),
        e!("Regex.source",     "r.source() -> String",                "Вихідний текст шаблону.", "r.source()"),
    ]},
    Section { title: "Типи та інтроспекція", entries: &[
        e!("typeOf", "typeOf(v: Value) -> String", "Назва типу значення: Number/String/Bool/Nil/List/Map/Result/Fn/Regex, або ім'я struct/enum.", "typeOf(42)"),
    ]},
    Section { title: "Процес", entries: &[
        e!("exit", "exit(code: Number) -> Nil", "Завершує процес з кодом виходу.", "exit(1)"),
        e!("args", "args() -> List<String>",    "Аргументи командного рядка.", "args()"),
        e!("env",  "env(name: String) -> Result<String, String>", "Змінна середовища.", "env(\"HOME\")?"),
    ]},
    Section { title: "Відладка", entries: &[
        e!("debug",   "debug(...) -> Nil",          "Друкує аргументи з типами в stderr (напр. 5 (Number)).", "debug(x, y)"),
        e!("assert",  "assert(cond: Bool, msg: String?) -> Nil", "Помилка виконання, якщо `cond` хибне.", "assert(x > 0, \"x має бути додатним\")"),
        e!("panic",   "panic(msg: String) -> Nil",  "Негайно завершує з помилкою виконання.", "panic(\"недосяжний код\")"),
        e!("gcstats", "gcstats() -> Nil",           "Друкує статистику GC в stdout. Для програмного доступу використовуйте gcStats().", "gcstats()"),
    ]},
    Section { title: "Збирач циклів", entries: &[
        e!("gcStats",         "gcStats() -> Map",                  "Статистика GC як Map: {allocs, drops, live}.", "let s = gcStats()"),
        e!("gcCollectCycles", "gcCollectCycles(...roots) -> Number", "Розриває цикли (struct/List/Map), недосяжні з переданих roots. Повертає кількість очищених об'єктів. У REPL запускається автоматично.", "gcCollectCycles(stillNeeded)"),
    ]},
    Section { title: "WeakRef", entries: &[
        e!("weakRef", "weakRef(v: Struct) -> WeakRef", "Слабке посилання — не заважає звичайному refcounting звільнити об'єкт.", "let w = weakRef(node)"),
        e!("upgrade", "upgrade(w: WeakRef) -> Result<Struct, String>", "Сильне посилання, якщо об'єкт ще живий, інакше Err.", "upgrade(w)?"),
        e!("isAlive", "isAlive(w: WeakRef) -> Bool", "Чи живий ще об'єкт, на який вказує слабке посилання.", "isAlive(w)"),
    ]},
    Section { title: "Модифікатори стійкості (у сигнатурі resilient fn)", entries: &[
        e!("deadline",          "deadline(N(ms|s|m))",               "Максимальний час виконання. Обов'язковий для resilient fn — компіляція падає без нього.", "deadline(500ms)"),
        e!("retryBudget",       "retryBudget(n: Number)",            "Скільки разів повторити виклик при помилці.", "retryBudget(3)"),
        e!("fallback",          "fallback(value)",                   "Значення, яке повернути, якщо всі спроби вичерпано.", "fallback(\"n/a\")"),
        e!("circuitBreaker",    "circuitBreaker(threshold: Number)", "Кількість підряд помилок, після якої виклики одразу відхиляються без спроб.", "circuitBreaker(5)"),
        e!("rateLimit",         "rateLimit(n: Number, per: N(ms|s|m))", "Не більше n викликів за період.", "rateLimit(10, per: 1s)"),
        e!("idempotent",        "idempotent(key: Expr)",             "Позначає виклик безпечним для повтору за цим ключем.", "idempotent(key: orderId)"),
        e!("emergencyFallback", "emergencyFallback(value)",          "Аварійне значення, коли навіть fallback недоступний.", "emergencyFallback(0)"),
        e!("timeout",           "timeout(N(ms|s|m))",                "Таймаут окремої спроби (на відміну від deadline — загального бюджету часу).", "timeout(200ms)"),
    ]},
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_documented_names_are_real_builtins() {
        // Методи (Regex.test, і т.п.) і модифікатори стійкості — не вільні
        // функції, is_builtin їх не знає; перевіряємо решту.
        for section in SECTIONS {
            if section.title.starts_with("Методи") || section.title.starts_with("Модифікатори") {
                continue;
            }
            for entry in section.entries {
                assert!(
                    crate::stdlib::is_builtin(entry.name),
                    "задокументовано '{}', але is_builtin() каже, що такої функції немає — \
                     перейменували чи видалили builtin і забули docs_data.rs?",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn no_duplicate_names_within_a_section() {
        for section in SECTIONS {
            let mut seen = std::collections::HashSet::new();
            for entry in section.entries {
                assert!(seen.insert(entry.name), "дублікат '{}' у секції '{}'", entry.name, section.title);
            }
        }
    }
}
