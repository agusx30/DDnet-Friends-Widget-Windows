// En Windows no abre ventana de consola: la app vive solo en la bandeja.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;
use std::{
    collections::HashSet,
    env, fs,
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

const SERVERS_URL: &str = "https://master1.ddnet.org/ddnet/15/servers.json";
const REFRESH_SECS: u64 = 45;
const MAX_MENU_ITEMS: usize = 25;

#[derive(Debug, Clone)]
struct Friend {
    name: String,
    clan: String,
}

#[derive(Debug, Clone)]
struct Online {
    name: String,
    clan: String,
    server: String,
    map: String,
}

type State = Result<Vec<Online>, String>;

enum UserEvent {
    Update(State),
    Menu(MenuEvent),
}

fn config_path() -> PathBuf {
    if let Some(arg) = env::args_os().nth(1) {
        return PathBuf::from(arg);
    }
    let appdata = env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    appdata.join("DDNet").join("settings_ddnet.cfg")
}

fn unescape(value: &str) -> String {
    value.replace("\\\\", "\\").replace("\\\"", "\"")
}

fn read_friends(path: &PathBuf) -> Result<Vec<Friend>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("No se pudo leer {}", path.display()))?;
    let re = Regex::new(
        r#"(?i)^\s*add_friend\s+"((?:\\.|[^"\\])*)"(?:\s+"((?:\\.|[^"\\])*)")?"#,
    )?;
    let mut friends = Vec::new();
    let mut seen = HashSet::new();
    for line in contents.lines() {
        if let Some(caps) = re.captures(line) {
            let name = unescape(&caps[1]).trim().to_string();
            let clan = caps
                .get(2)
                .map(|m| unescape(m.as_str()).trim().to_string())
                .unwrap_or_default();
            if !name.is_empty() && seen.insert((name.to_lowercase(), clan.to_lowercase())) {
                friends.push(Friend { name, clan });
            }
        }
    }
    Ok(friends)
}

fn str_at<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn map_name(info: &Value) -> String {
    match info.get("map") {
        Some(Value::String(s)) => s.clone(),
        Some(m) => str_at(m, "name").to_string(),
        None => String::new(),
    }
}

fn fetch_online(friends: &[Friend]) -> Result<Vec<Online>> {
    let root: Value = ureq::get(SERVERS_URL)
        .timeout(Duration::from_secs(20))
        .call()
        .context("No se pudo conectar con la lista maestra de DDNet")?
        .into_json()
        .context("Respuesta de la lista maestra no válida")?;

    let servers = root
        .get("servers")
        .and_then(Value::as_array)
        .or_else(|| root.as_array())
        .context("Formato de la lista maestra no reconocido")?;

    let mut found = Vec::new();
    let mut seen = HashSet::new();

    for server in servers {
        // Los datos suelen venir bajo "info"; si no, se leen del nivel superior.
        let info = server.get("info").unwrap_or(server);
        let Some(clients) = info.get("clients").and_then(Value::as_array) else {
            continue;
        };
        let server_name = str_at(info, "name").to_string();
        let map = map_name(info);

        for player in clients {
            let pname = str_at(player, "name");
            let pclan = str_at(player, "clan");
            let matched = friends.iter().any(|f| {
                f.name.eq_ignore_ascii_case(pname)
                    && (f.clan.is_empty() || f.clan.eq_ignore_ascii_case(pclan))
            });
            if matched && seen.insert(pname.to_lowercase()) {
                found.push(Online {
                    name: pname.to_string(),
                    clan: pclan.to_string(),
                    server: server_name.clone(),
                    map: map.clone(),
                });
            }
        }
    }
    found.sort_by_key(|o| o.name.to_lowercase());
    Ok(found)
}

fn worker(proxy: EventLoopProxy<UserEvent>, rx: mpsc::Receiver<()>, config: PathBuf) {
    let mut previous: Option<HashSet<String>> = None;
    loop {
        let result: Result<Vec<Online>> = read_friends(&config).and_then(|friends| {
            if friends.is_empty() {
                anyhow::bail!("No hay líneas add_friend en la configuración");
            }
            fetch_online(&friends)
        });

        let state: State = match result {
            Ok(list) => {
                let current: HashSet<String> =
                    list.iter().map(|o| o.name.to_lowercase()).collect();
                if let Some(prev) = &previous {
                    for o in list.iter().filter(|o| !prev.contains(&o.name.to_lowercase())) {
                        let _ = notify_rust::Notification::new()
                            .appname("DDNet Friends")
                            .summary(&format!("{} se conectó", o.name))
                            .body(&format!("{} · {}", o.map, o.server))
                            .show();
                    }
                }
                previous = Some(current);
                Ok(list)
            }
            Err(e) => Err(format!("{e:#}")),
        };

        if proxy.send_event(UserEvent::Update(state)).is_err() {
            break;
        }
        match rx.recv_timeout(Duration::from_secs(REFRESH_SECS)) {
            Err(RecvTimeoutError::Disconnected) => break,
            _ => {}
        }
    }
}

fn make_icon() -> Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - 16.0;
            let dy = y as f32 + 0.5 - 16.0;
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = ((15.0 - d).clamp(0.0, 1.0) * 255.0) as u8;
            rgba.extend_from_slice(&[140, 140, 140, alpha]); // gris fijo
        }
    }
    Icon::from_rgba(rgba, size, size).expect("icono inválido")
}

fn build_menu(state: Option<&State>) -> Menu {
    let menu = Menu::new();
    let header = match state {
        None => "Cargando...".to_string(),
        Some(Err(e)) => format!("Error: {e}"),
        Some(Ok(list)) if list.is_empty() => "Ningún amigo conectado".to_string(),
        Some(Ok(list)) => format!("Amigos conectados: {}", list.len()),
    };
    let _ = menu.append(&MenuItem::new(header, false, None));
    if let Some(Ok(list)) = state {
        if !list.is_empty() {
            let _ = menu.append(&PredefinedMenuItem::separator());
            for o in list.iter().take(MAX_MENU_ITEMS) {
                let clan = if o.clan.is_empty() { String::new() } else { format!(" [{}]", o.clan) };
                let text = format!("{}{} — {} ({})", o.name, clan, o.map, o.server);
                let _ = menu.append(&MenuItem::new(text, false, None));
            }
        }
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("refresh", "Actualizar ahora", true, None));
    let _ = menu.append(&MenuItem::with_id("quit", "Salir", true, None));
    menu
}

fn tooltip(state: &State) -> String {
    let text = match state {
        Err(e) => format!("DDNet Friends: {e}"),
        Ok(list) if list.is_empty() => "DDNet Friends: nadie conectado".to_string(),
        Ok(list) => {
            let names: Vec<&str> = list.iter().map(|o| o.name.as_str()).collect();
            names.join(", ")
        }
    };
    // Windows limita el tooltip a ~127 caracteres.
    text.chars().take(120).collect()
}

fn main() {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(build_menu(None)))
        .with_tooltip("DDNet Friends: cargando...")
        .with_icon(make_icon())
        .build()
        .expect("no se pudo crear el ícono de bandeja");

    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    let cfg = config_path();
    thread::spawn(move || worker(proxy, refresh_rx, cfg));

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Update(state)) => {
                let _ = tray.set_tooltip(Some(tooltip(&state)));
                tray.set_menu(Some(Box::new(build_menu(Some(&state)))));
            }
            Event::UserEvent(UserEvent::Menu(e)) => match e.id.0.as_str() {
                "quit" => *control_flow = ControlFlow::Exit,
                "refresh" => {
                    let _ = refresh_tx.send(());
                }
                _ => {}
            },
            _ => {}
        }
    });
}
