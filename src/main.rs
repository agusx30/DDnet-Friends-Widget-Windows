// En Windows no abre ventana de consola: la app vive solo en la bandeja.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AppConfig {
    ddnet_exe: Option<PathBuf>,
}

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
    address: String,
}

type State = Result<Vec<Online>, String>;

enum UserEvent {
    Update(State),
    Menu(MenuEvent),
    ConfigChanged(AppConfig),
}

fn config_path() -> PathBuf {
    if let Some(arg) = env::args_os().nth(1) {
        return PathBuf::from(arg);
    }
    let appdata = env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    appdata.join("DDNet").join("settings_ddnet.cfg")
}

fn app_config_dir() -> PathBuf {
    let appdata = env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    appdata.join("ddnet-friends")
}

fn app_config_path() -> PathBuf {
    app_config_dir().join("config.json")
}

fn detect_ddnet_exe() -> Option<PathBuf> {
    // 1. Consultar registro de Windows para el protocolo ddnet://
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW = 0x08000000
        if let Ok(output) = std::process::Command::new("reg")
            .args(["query", "HKCR\\ddnet\\shell\\open\\command", "/ve"])
            .creation_flags(0x08000000)
            .output()
        {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    if let Some(pos) = line.find("REG_SZ") {
                        let val = line[pos + 6..].trim();
                        let candidate_str = if let Some(stripped) = val.strip_prefix('"') {
                            stripped.split('"').next().unwrap_or("").trim()
                        } else {
                            val.split_whitespace().next().unwrap_or("").trim()
                        };
                        let candidate = PathBuf::from(candidate_str.replace('/', "\\"));
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
            }
        }
    }

    // 2. Rutas habituales de instalación
    let candidates = [
        r"C:\Program Files\DDNet\DDNet.exe",
        r"C:\Program Files (x86)\DDNet\DDNet.exe",
        r"C:\Program Files (x86)\Steam\steamapps\common\DDNet\DDNet.exe",
        r"C:\Program Files\Steam\steamapps\common\DDNet\DDNet.exe",
    ];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_file() {
            return Some(p);
        }
    }

    None
}

fn load_app_config() -> AppConfig {
    let path = app_config_path();
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&data) {
            return cfg;
        }
    }

    // Si no existía configuración previa, intentar autodetectar la ruta
    let mut cfg = AppConfig::default();
    if let Some(detected) = detect_ddnet_exe() {
        cfg.ddnet_exe = Some(detected);
        let _ = save_app_config(&cfg);
    }
    cfg
}

fn save_app_config(cfg: &AppConfig) -> Result<()> {
    let dir = app_config_dir();
    fs::create_dir_all(&dir)?;
    let data = serde_json::to_string_pretty(cfg)?;
    fs::write(app_config_path(), data)?;
    Ok(())
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

fn extract_address(server: &Value) -> String {
    let addresses = server.get("addresses").and_then(Value::as_array);
    if let Some(list) = addresses {
        let raw = list
            .iter()
            .find_map(|v| {
                let s = v.as_str()?;
                if s.starts_with("tw-0.6+udp://") {
                    Some(s)
                } else {
                    None
                }
            })
            .or_else(|| list.first().and_then(Value::as_str));

        if let Some(raw_str) = raw {
            if let Some((_, host_port)) = raw_str.split_once("://") {
                return host_port.to_string();
            }
            return raw_str.to_string();
        }
    }
    String::new()
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
        let address = extract_address(server);

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
                    address: address.clone(),
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

fn make_icon(has_online: bool) -> Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let (r, g, b) = if has_online {
        (0u8, 180u8, 245u8) // Celeste DDNet brillante
    } else {
        (130u8, 130u8, 130u8) // Gris neutro
    };
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - 16.0;
            let dy = y as f32 + 0.5 - 16.0;
            let d = (dx * dx + dy * dy).sqrt();
            let alpha = ((14.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
            rgba.extend_from_slice(&[r, g, b, alpha]);
        }
    }
    Icon::from_rgba(rgba, size, size).expect("icono inválido")
}

fn build_menu(state: Option<&State>, config: &AppConfig) -> Menu {
    let menu = Menu::new();
    let header = match state {
        None => "Cargando...".to_string(),
        Some(Err(e)) => format!("Error: {e}"),
        Some(Ok(list)) if list.is_empty() => "Ningún amigo conectado".to_string(),
        Some(Ok(list)) => format!("Amigos conectados ({}) — clic para unirte:", list.len()),
    };
    let _ = menu.append(&MenuItem::new(header, false, None));
    if let Some(Ok(list)) = state {
        if !list.is_empty() {
            let _ = menu.append(&PredefinedMenuItem::separator());
            for (idx, o) in list.iter().take(MAX_MENU_ITEMS).enumerate() {
                let clan = if o.clan.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", o.clan)
                };
                let text = format!("{}{} — {} ({})", o.name, clan, o.map, o.server);
                if !o.address.is_empty() {
                    let _ = menu.append(&MenuItem::with_id(
                        format!("join:{}:{}", idx, o.address),
                        format!("🎮 {text}"),
                        true,
                        None,
                    ));
                } else {
                    let _ = menu.append(&MenuItem::new(text, false, None));
                }
            }
        }
    }
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("refresh", "Actualizar ahora", true, None));

    // Configuración de la ruta del ejecutable del juego
    let _ = menu.append(&PredefinedMenuItem::separator());
    if let Some(path) = &config.ddnet_exe {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("DDNet.exe");
        let _ = menu.append(&MenuItem::new(format!("Juego: {name}"), false, None));
        let _ = menu.append(&MenuItem::with_id(
            "config_exe",
            "Cambiar ruta de DDNet.exe...",
            true,
            None,
        ));
    } else {
        let _ = menu.append(&MenuItem::with_id(
            "config_exe",
            "Configurar ruta de DDNet.exe...",
            true,
            None,
        ));
    }

    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&MenuItem::with_id("quit", "Salir", true, None));
    menu
}

fn tooltip(state: &State) -> String {
    match state {
        Err(e) => e.clone(),
        Ok(list) if list.is_empty() => "Nadie conectado".to_string(),
        Ok(list) => {
            let names: Vec<&str> = list.iter().map(|o| o.name.as_str()).collect();
            let full = names.join(", ");
            // En Windows, szTip soporta hasta 127 caracteres.
            if full.chars().count() <= 127 {
                return full;
            }

            // Si supera el límite de 127 caracteres, incluimos la mayor cantidad de nombres completos posibles
            // sin cortar nombres por la mitad ni dejar comas colgando.
            let mut result = String::new();
            let mut included = 0;
            for (i, name) in names.iter().enumerate() {
                let remaining = names.len() - i;
                let suffix = if remaining > 0 {
                    format!(" y {remaining} más")
                } else {
                    String::new()
                };
                let candidate = if result.is_empty() {
                    name.to_string()
                } else {
                    format!("{result}, {name}")
                };
                if (candidate.chars().count() + suffix.chars().count()) <= 127 {
                    result = candidate;
                    included += 1;
                } else {
                    break;
                }
            }
            let leftover = names.len() - included;
            if leftover > 0 {
                format!("{result} y {leftover} más")
            } else {
                result
            }
        }
    }
}

#[cfg(windows)]
fn set_tray_tooltip(tip: &str) {
    use windows_sys::{
        s,
        Win32::UI::{
            Shell::{Shell_NotifyIconW, NIF_TIP, NIM_MODIFY, NOTIFYICONDATAW},
            WindowsAndMessaging::FindWindowA,
        },
    };

    unsafe {
        let hwnd = FindWindowA(s!("tray_icon_app"), std::ptr::null());
        if !hwnd.is_null() {
            let mut wide: Vec<u16> = tip.encode_utf16().collect();
            wide.truncate(127);
            wide.push(0);

            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = NIF_TIP;
            for (i, &c) in wide.iter().enumerate().take(128) {
                nid.szTip[i] = c;
            }
            Shell_NotifyIconW(NIM_MODIFY, &mut nid as _);
        }
    }
}

fn join_server(address: &str, custom_exe: Option<&PathBuf>) {
    let url = format!("ddnet://{}", address);

    let exe_to_run = custom_exe
        .filter(|p| p.is_file())
        .cloned()
        .or_else(detect_ddnet_exe);

    if let Some(exe) = exe_to_run {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg(&url);
        if let Some(parent) = exe.parent() {
            cmd.current_dir(parent);
        }
        match cmd.spawn() {
            Ok(_) => {
                let _ = notify_rust::Notification::new()
                    .appname("DDNet Friends")
                    .summary("Conectando al servidor...")
                    .body(&format!("Uniendo a {}", address))
                    .show();
            }
            Err(e) => {
                let _ = notify_rust::Notification::new()
                    .appname("DDNet Friends")
                    .summary("Error al ejecutar DDNet")
                    .body(&format!("Falló al iniciar {}: {}", exe.display(), e))
                    .show();
            }
        }
    } else {
        // Fallback al protocolo registrado en el sistema operativo
        let res = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn();

        if let Err(_) = res {
            let _ = notify_rust::Notification::new()
                .appname("DDNet Friends")
                .summary("DDNet no encontrado")
                .body("Configurá la ruta del ejecutable en el menú de la bandeja.")
                .show();
        }
    }
}

fn main() {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));

    let mut app_config = load_app_config();
    let mut last_state: Option<State> = None;

    let mut tray_res = None;
    for attempt in 1..=5 {
        match TrayIconBuilder::new()
            .with_menu(Box::new(build_menu(None, &app_config)))
            .with_menu_on_left_click(true)
            .with_tooltip("Cargando...")
            .with_icon(make_icon(false))
            .build()
        {
            Ok(t) => {
                tray_res = Some(t);
                break;
            }
            Err(e) => {
                eprintln!("Intento {attempt} falló: {e:?}");
                thread::sleep(Duration::from_millis(300));
            }
        }
    }
    let tray = tray_res.expect("no se pudo crear el ícono de bandeja tras reintentos");

    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();
    let cfg = config_path();
    let worker_proxy = proxy.clone();
    thread::spawn(move || worker(worker_proxy, refresh_rx, cfg));

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Update(state)) => {
                let has_online = matches!(&state, Ok(list) if !list.is_empty());
                let _ = tray.set_icon(Some(make_icon(has_online)));
                let tip = tooltip(&state);
                let _ = tray.set_tooltip(Some(&tip));
                #[cfg(windows)]
                set_tray_tooltip(&tip);
                tray.set_menu(Some(Box::new(build_menu(Some(&state), &app_config))));
                last_state = Some(state);
            }
            Event::UserEvent(UserEvent::ConfigChanged(cfg)) => {
                app_config = cfg;
                tray.set_menu(Some(Box::new(build_menu(last_state.as_ref(), &app_config))));
            }
            Event::UserEvent(UserEvent::Menu(e)) => match e.id.0.as_str() {
                "quit" => *control_flow = ControlFlow::Exit,
                "refresh" => {
                    let _ = refresh_tx.send(());
                }
                "config_exe" => {
                    let proxy_clone = proxy.clone();
                    let current_path = app_config.ddnet_exe.clone();
                    thread::spawn(move || {
                        let mut dialog = rfd::FileDialog::new()
                            .set_title("Seleccionar ejecutable de DDNet (DDNet.exe)")
                            .add_filter("Ejecutable DDNet (*.exe)", &["exe"]);
                        if let Some(ref curr) = current_path {
                            if let Some(parent) = curr.parent() {
                                dialog = dialog.set_directory(parent);
                            }
                        }
                        if let Some(picked) = dialog.pick_file() {
                            let mut cfg = load_app_config();
                            cfg.ddnet_exe = Some(picked.clone());
                            let _ = save_app_config(&cfg);
                            let _ = notify_rust::Notification::new()
                                .appname("DDNet Friends")
                                .summary("Ruta guardada")
                                .body(&format!("{}", picked.display()))
                                .show();
                            let _ = proxy_clone.send_event(UserEvent::ConfigChanged(cfg));
                        }
                    });
                }
                id if id.starts_with("join:") => {
                    let parts: Vec<&str> = id.splitn(3, ':').collect();
                    if parts.len() == 3 {
                        let addr = parts[2];
                        join_server(addr, app_config.ddnet_exe.as_ref());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    });
}
