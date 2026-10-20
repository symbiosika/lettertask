//! Untrusted file names (mail attachments) on the local file system: safe
//! names for saving, and which files must never be opened with one click.

/// Longest file name we produce, in UTF-8 bytes (ext4/NTFS allow 255).
const MAX_NAME_BYTES: usize = 200;

/// File types the OS would execute (or mount/install) instead of just showing
/// them. Opening those straight from a mail is how malware gets started.
const RISKY_EXTENSIONS: &[&str] = &[
    // Windows executables, installers, scripts, shortcuts
    "exe", "com", "scr", "pif", "cpl", "msi", "msp", "mst", "msc", "bat", "cmd", "ps1", "ps1xml", "ps2", "ps2xml",
    "psc1", "psc2", "psd1", "psm1", "vb", "vbs", "vbe", "js", "jse", "wsf", "wsh", "ws", "wsc", "sct", "hta", "jar",
    "lnk", "url", "scf", "inf", "reg", "dll", "sys", "ocx", "application", "appref-ms", "gadget", "msh", "msh1",
    "msh2", "mshxml", "msh1xml", "msh2xml", "diagcab", "settingcontent-ms", "library-ms", "search-ms",
    "searchconnector-ms", "chm", "hlp", "appx", "appxbundle", "msix", "msixbundle", "xll", "xlam", "ppam", "iqy",
    "slk", "website", "cab",
    // disk images (auto-mount, bypass "downloaded from the internet" marks)
    "iso", "img", "vhd", "vhdx", "dmg",
    // macOS / Linux
    "app", "command", "tool", "terminal", "workflow", "action", "scpt", "applescript", "pkg", "mpkg", "sh", "bash",
    "zsh", "csh", "ksh", "desktop", "run", "bin", "appimage", "deb", "rpm", "py", "pyw", "pl", "rb",
];

/// Windows device names that cannot be used as file names (with any extension).
fn is_reserved_windows_name(stem: &str) -> bool {
    let s = stem.trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(s.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((s.starts_with("COM") || s.starts_with("LPT"))
            && s.len() == 4
            && s.as_bytes()[3].is_ascii_digit())
}

/// Invisible characters that reorder or hide text: "rechnung\u{202E}fdp.exe"
/// is displayed as "rechnungexe.pdf".
fn is_invisible_format_char(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

/// Makes an attachment name safe to use as a single file name on Windows,
/// macOS and Linux: no path separators or reserved characters, no control or
/// bidi characters, no hidden/dot names, no trailing dots or spaces (Windows
/// drops them, so "x.exe." would become "x.exe"), no device names, bounded length.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !is_invisible_format_char(*c))
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let mut name = cleaned
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .trim()
        .to_string();
    if name.len() > MAX_NAME_BYTES {
        name = truncate_keeping_extension(&name);
    }
    let stem = name.split('.').next().unwrap_or("");
    if is_reserved_windows_name(stem) {
        name.insert(0, '_');
    }
    if name.is_empty() {
        "anhang".into()
    } else {
        name
    }
}

fn truncate_keeping_extension(name: &str) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 17 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let mut end = MAX_NAME_BYTES - ext.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", stem[..end].trim_end_matches(['.', ' ']), ext)
}

/// True if handing this file to the OS "open" action could run code.
pub fn is_risky_to_open(file_name: &str) -> bool {
    let name = file_name.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((_, ext)) => RISKY_EXTENSIONS.contains(&ext),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_safe() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_filename("Rechnung: 10/2026.pdf"), "Rechnung_ 10_2026.pdf");
        assert_eq!(sanitize_filename(""), "anhang");
        assert_eq!(sanitize_filename(".."), "anhang");
        assert_eq!(sanitize_filename("/etc/passwd"), "_etc_passwd");
        assert_eq!(sanitize_filename("C:\\Windows\\system32\\x.dll"), "C__Windows_system32_x.dll");
        assert_eq!(sanitize_filename("a\0b.txt"), "a_b.txt");
        assert_eq!(sanitize_filename("  .bashrc  "), "bashrc");
    }

    #[test]
    fn windows_quirks_are_neutralised() {
        // trailing dots/spaces are dropped by Windows: "x.exe." would run as x.exe
        assert_eq!(sanitize_filename("rechnung.exe. . "), "rechnung.exe");
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_filename("com1.pdf"), "_com1.pdf");
        assert_eq!(sanitize_filename("Lpt9"), "_Lpt9");
        assert_eq!(sanitize_filename("console.txt"), "console.txt");
        assert_eq!(sanitize_filename("com10.txt"), "com10.txt");
    }

    #[test]
    fn invisible_and_bidi_characters_are_removed() {
        assert_eq!(sanitize_filename("rechnung\u{202E}fdp.exe"), "rechnungfdp.exe");
        assert_eq!(sanitize_filename("a\u{200B}b\u{FEFF}.pdf"), "ab.pdf");
        assert_eq!(sanitize_filename("Übersicht – März.pdf"), "Übersicht – März.pdf");
    }

    #[test]
    fn long_names_are_cut_but_keep_their_extension() {
        let long = format!("{}.pdf", "ä".repeat(300));
        let s = sanitize_filename(&long);
        assert!(s.len() <= MAX_NAME_BYTES, "{}", s.len());
        assert!(s.ends_with(".pdf"));
        let no_ext = "x".repeat(1000);
        assert_eq!(sanitize_filename(&no_ext).len(), MAX_NAME_BYTES);
        // the cut must not leave a trailing dot that Windows would drop
        let dotted = format!("{}.{}", "a".repeat(195), "b".repeat(40));
        assert!(!sanitize_filename(&dotted).ends_with('.'));
    }

    #[test]
    fn executables_are_risky() {
        for n in ["setup.exe", "RECHNUNG.PDF.EXE", "x.js", "a.lnk", "b.hta", "c.ps1", "d.iso", "e.exe.", "f.exe  ", "g.desktop", "h.sh"] {
            assert!(is_risky_to_open(n), "{n}");
        }
        for n in ["rechnung.pdf", "foto.jpg", "brief.docx", "tabelle.xlsx", "README", "archiv.zip", "exe"] {
            assert!(!is_risky_to_open(n), "{n}");
        }
    }
}
