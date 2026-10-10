//! `jw theme`: the themes without the TUI (S16). Lists them, installs one
//! from a file, stdin or a URL, prints one as a template, or picks one for
//! dark or light mode; a running TUI applies the change within 2 s (REQ-69).

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use jw_tui::{settings, theme};

const USAGE: &str = "usage: jw theme [ls | install <file|url|-> [--name <name>] [--force] | export <name> | use <name>]";

pub fn run(args: &[String]) -> Result<()> {
    match args {
        [] => ls(),
        [a] if a == "ls" => ls(),
        [a, name] if a == "export" => {
            print!("{}", theme::export(name)?);
            Ok(())
        }
        [a, name] if a == "use" => pick(name),
        [a, from, rest @ ..] if a == "install" => {
            let (mut name, mut force) = (None, false);
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--force" => force = true,
                    "--name" => name = Some(rest.next().context(USAGE)?.as_str()),
                    _ => bail!(USAGE),
                }
            }
            let (data, stem) = fetch(from)?;
            let path = theme::install(&data, name, stem.as_deref(), force)?;
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            let replaces = match theme::source(&name) {
                "file, replaces built-in" => " (it replaces the built-in one)",
                _ => "",
            };
            println!(
                "installed {}{replaces} · jw theme use {name} picks it",
                path.display()
            );
            Ok(())
        }
        _ => bail!(USAGE),
    }
}

/// REQ-108: each theme, where it comes from, dark or light, and the slot it
/// fills.
fn ls() -> Result<()> {
    let s = settings::read(&settings::path()?)?;
    for name in theme::names() {
        let mode = match theme::named(&name) {
            Ok(p) if p.dark => "dark",
            Ok(_) => "light",
            Err(_) => "doesn't load",
        };
        let used = match (name == s.dark_theme(), name == s.light_theme()) {
            (true, true) => "  ← dark and light",
            (true, false) => "  ← dark",
            (false, true) => "  ← light",
            _ => "",
        };
        println!("{name:<20} {mode:<6} {}{used}", theme::source(&name));
    }
    Ok(())
}

/// REQ-107: the theme goes in the slot its `dark` says.
fn pick(name: &str) -> Result<()> {
    let dark = theme::named(name)?.dark;
    let mut s = settings::read(&settings::path()?)?;
    if dark {
        s.dark = Some(name.to_string());
    } else {
        s.light = Some(name.to_string());
    }
    settings::save(s)?;
    println!(
        "{name} is the {} theme",
        if dark { "dark" } else { "light" }
    );
    Ok(())
}

/// The bytes of a theme and the name its file suggests: a path, `-` for
/// stdin, or an http(s) URL fetched with curl.
fn fetch(from: &str) -> Result<(Vec<u8>, Option<String>)> {
    let stem = |p: &str| {
        Path::new(p.split(['?', '#']).next().unwrap_or(p))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    };
    if from == "-" {
        let mut data = Vec::new();
        std::io::stdin().read_to_end(&mut data)?;
        return Ok((data, None));
    }
    if from.starts_with("http://") || from.starts_with("https://") {
        let out = std::process::Command::new("curl")
            .args(["-fsSL", "--max-time", "30", "--", from])
            .output()
            .context("running curl")?;
        if !out.status.success() {
            bail!(
                "curl {from}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        return Ok((out.stdout, stem(from)));
    }
    let data = std::fs::read(from).with_context(|| format!("reading {from}"))?;
    Ok((data, stem(from)))
}
