//! `jwn pair`, `jwn devices`, `jwn revoke`: the PC's side of the phone link (REQ-75, REQ-78).

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use contract::Cmd;
use daemon::{Config, devices_file};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;
use web::auth::Store;
use wire::Client;

type Res = Result<ExitCode, String>;

fn connect() -> Result<Client, String> {
    let socket = identity::socket_path();
    let mut client = Client::connect(&socket, "jwn-pair").map_err(|e| {
        format!(
            "cannot reach the daemon on {}: {e} (is `jwn daemon` running?)",
            socket.display()
        )
    })?;
    let root = client.root.clone();
    client.open(&root).map_err(|e| e.to_string())?;
    Ok(client)
}

fn run(client: &mut Client, cmd: Cmd) -> Result<Option<serde_json::Value>, String> {
    run_within(client, cmd, Duration::from_secs(30))
}

fn run_within(
    client: &mut Client,
    cmd: Cmd,
    wait: Duration,
) -> Result<Option<serde_json::Value>, String> {
    client
        .cmd_within(cmd, wait)
        .map_err(|e| e.to_string())?
        .map_err(|e| e.message)
}

/// Turn the link on, show the QR, and answer one pairing question.
pub fn pair() -> Res {
    let mut client = connect()?;
    println!("Abriendo el túnel… (puede tardar hasta un minuto)");
    // The tunnel may take about a minute to resolve (the daemon waits up to 90 s).
    let started = run_within(&mut client, Cmd::MobileStart {}, Duration::from_secs(120))?
        .ok_or("the daemon gave no link")?;
    let uri = started["uri"].as_str().ok_or("the daemon gave no link")?;
    if let Ok(code) = QrCode::new(uri) {
        println!("{}", code.render::<Dense1x2>().quiet_zone(true).build());
    }
    println!("{uri}\n");
    for (what, key) in [("tunnel", "tunnel"), ("red local", "lan")] {
        if let Some(addr) = started[key].as_str() {
            println!("{what}: {addr}");
        }
    }
    println!(
        "\nEscanea el código o pega el enlace en la app del teléfono. Esperando… (Ctrl-C para salir; el enlace sigue activo)"
    );
    loop {
        let Some(ask) = run(&mut client, Cmd::MobileWait { seconds: 25 })? else {
            continue;
        };
        let (name, code) = (
            ask["name"].as_str().unwrap_or("?"),
            ask["code"].as_str().unwrap_or("?"),
        );
        print!(
            "\n¿Emparejar «{name}»? El teléfono muestra el código {code}. Solo «y» acepta [y/N] "
        );
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let yes = line.trim().eq_ignore_ascii_case("y");
        run(&mut client, Cmd::MobileAnswer { yes })?;
        println!("{}", if yes { "Emparejado." } else { "Rechazado." });
        return Ok(ExitCode::SUCCESS);
    }
}

pub fn stop() -> Res {
    run(&mut connect()?, Cmd::MobileStop {})?;
    println!("El enlace con el teléfono está apagado.");
    Ok(ExitCode::SUCCESS)
}

fn store() -> Store {
    Store::new(devices_file(&Config::from_identity().state_dir))
}

fn ago(secs: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let d = Duration::from_secs(now.saturating_sub(secs));
    match d.as_secs() {
        0..=59 => "ahora".into(),
        60..=3599 => format!("hace {} min", d.as_secs() / 60),
        3600..=86399 => format!("hace {} h", d.as_secs() / 3600),
        s => format!("hace {} d", s / 86400),
    }
}

pub fn devices() -> Res {
    let devices = store().list().map_err(|e| e.to_string())?;
    if devices.is_empty() {
        println!("No hay teléfonos emparejados.");
    }
    for d in devices {
        println!(
            "{}  {:<24} emparejado {}  visto {}",
            d.id,
            d.name,
            ago(d.created),
            ago(d.last_seen)
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn revoke(which: &str) -> Res {
    if store().revoke(which).map_err(|e| e.to_string())? {
        println!("«{which}» ya no puede conectarse.");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!("no hay un teléfono «{which}» (mira `jwn devices`)"))
    }
}
