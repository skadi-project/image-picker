use std::cmp::Ordering;
use std::env;
use std::fs;
use std::iter::Peekable;
use std::str::Chars;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const IMAGE_EXTENSIONS: &[&str] = &[
    "webp",
    // "jpg", "jpeg", "png", "gif", "bmp", "webp", "tif", "tiff", "ico", "svg",
];

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Рекурсивно собирает изображения вместе с датой изменения.
fn scan(dir: &Path, out: &mut Vec<(PathBuf, SystemTime)>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Не удалось прочитать {}: {}", dir.display(), e);
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // file_type() не следует по симлинкам — защита от циклов
        let Ok(file_type) = entry.file_type() else { continue };

        if file_type.is_dir() {
            scan(&path, out);
        } else if file_type.is_file() && is_image(&path) {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                out.push((path, modified));
            }
        }
    }
}

/// Читает из итератора подряд идущие цифры.
fn take_number(it: &mut Peekable<Chars>) -> String {
    let mut s = String::new();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            s.push(c);
            it.next();
        } else {
            break;
        }
    }
    s
}

/// «Естественное» сравнение строк: числа сравниваются по значению,
/// поэтому "2" < "10", а регистр букв не учитывается.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();

    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_number(&mut ai);
                    let nb = take_number(&mut bi);
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    let la = ca.to_lowercase().next().unwrap_or(ca);
                    let lb = cb.to_lowercase().next().unwrap_or(cb);
                    if la != lb {
                        return la.cmp(&lb);
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

/// Сравнивает пути покомпонентно (папка за папкой, затем файл),
/// относительно корневой директории.
fn path_cmp(a: &Path, b: &Path, root: &Path) -> Ordering {
    let ra = a.strip_prefix(root).unwrap_or(a);
    let rb = b.strip_prefix(root).unwrap_or(b);
    let mut ca = ra.components();
    let mut cb = rb.components();

    loop {
        match (ca.next(), cb.next()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = natural_cmp(
                    &x.as_os_str().to_string_lossy(),
                    &y.as_os_str().to_string_lossy(),
                );
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Форматирует время как "ГГГГ-ММ-ДД ЧЧ:ММ:СС" (UTC).
fn format_time(t: SystemTime) -> String {
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };

    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, m, s)
}

/// Открывает файл программой по умолчанию.
fn open_file(path: &Path) -> io::Result<()> {
    if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(path)
            .spawn()?;
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()?;
    } else {
        Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

fn main() {
    let root = env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = PathBuf::from(root);

    if !root.is_dir() {
        eprintln!("'{}' не является директорией", root.display());
        std::process::exit(1);
    }

    let mut files = Vec::new();
    scan(&root, &mut files);

    if files.is_empty() {
        println!("Изображения не найдены в {}", root.display());
        return;
    }

    // Сначала по имени папки, затем по имени файла (по возрастанию,
    // числа сравниваются по значению: 2 идёт перед 10)
    files.sort_by(|a, b| path_cmp(&a.0, &b.0, &root));

    for (i, (path, time)) in files.iter().enumerate() {
        println!("{:>4}. [{}] {}", i + 1, format_time(*time), path.display());
    }

    let total = files.len();
    loop {
        print!("\nВведите номер (1-{}), q или пустая строка — выход: ", total);
        io::stdout().flush().ok();

        let mut input = String::new();
        match io::stdin().read_line(&mut input) {
            Ok(0) => return, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("Ошибка ввода: {}", e);
                return;
            }
        }

        let input = input.trim();
        if input.is_empty() || input.eq_ignore_ascii_case("q") {
            return;
        }

        match input.parse::<usize>() {
            Ok(n) if (1..=total).contains(&n) => {
                let path = &files[n - 1].0;
                println!("Открываю: {}", path.display());
                if let Err(e) = open_file(path) {
                    eprintln!("Не удалось открыть файл: {}", e);
                }
            }
            _ => println!("Нужно целое число от 1 до {}.", total),
        }
    }
}
