use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

const CLI_PATH: &str = "/usr/bin/zerotier-cli";

#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub address: String,
    pub version: String,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub status: String,
    pub ip: String,
    pub mac: String,
    pub allow_managed: bool,
    pub allow_global: bool,
    pub allow_default: bool,
    pub allow_dns: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub ip: String,
    pub latency: i64,
    pub is_online: bool,
    pub mac: String,
}

fn get_user_token_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".zeroTierOneAuthToken"))
}

pub fn has_auth_token() -> bool {
    get_user_token_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn setup_auth_token() -> Result<String, String> {
    let token_path = get_user_token_path().ok_or("Не удалось определить домашнюю папку")?;
    let token_str = token_path.to_string_lossy().to_string();
    let user = gtk4::glib::user_name().to_string_lossy().to_string();

    let script = format!(
        "cp /var/lib/zerotier-one/authtoken.secret '{token_str}' && chown {user}:{user} '{token_str}' && chmod 600 '{token_str}'"
    );

    let output = Command::new("pkexec")
    .args(["sh", "-c", &script])
    .output()
    .map_err(|e| format!("Ошибка pkexec: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if !err.is_empty() { err } else { "Отменено пользователем".into() });
    }

    Ok("Токен доступа успешно настроен!".into())
}

pub fn is_service_active() -> bool {
    Command::new("systemctl")
    .args(["is-active", "--quiet", "zerotier-one.service"])
    .status()
    .map(|s| s.success())
    .unwrap_or(false)
}

pub fn is_service_enabled() -> bool {
    Command::new("systemctl")
    .args(["is-enabled", "--quiet", "zerotier-one.service"])
    .status()
    .map(|s| s.success())
    .unwrap_or(false)
}

pub fn set_service_enabled(enable: bool) -> Result<String, String> {
    let action = if enable { "enable" } else { "disable" };
    let output = Command::new("pkexec")
    .args(["systemctl", action, "zerotier-one.service"])
    .output()
    .map_err(|e| format!("Ошибка pkexec: {e}"))?;

    if !output.status.success() {
        return Err("Не удалось изменить статус автозапуска".into());
    }

    Ok(if enable {
        "Автозапуск службы включен"
    } else {
        "Автозапуск службы выключен"
    }
    .into())
}

pub fn start_service() -> Result<String, String> {
    let output = Command::new("pkexec")
    .args(["systemctl", "start", "zerotier-one.service"])
    .output()
    .map_err(|e| format!("Не удалось выполнить команду: {e}"))?;

    if !output.status.success() {
        return Err("Не удалось запустить zerotier-one.service".into());
    }
    Ok("Служба ZeroTier запущена".into())
}

pub fn stop_service() -> Result<String, String> {
    let output = Command::new("pkexec")
    .args(["systemctl", "stop", "zerotier-one.service"])
    .output()
    .map_err(|e| format!("Не удалось выполнить команду: {e}"))?;

    if !output.status.success() {
        return Err("Не удалось остановить zerotier-one.service".into());
    }
    Ok("Служба ZeroTier остановлена".into())
}

fn run_cli(args: &[&str]) -> Result<String, String> {
    if let Ok(output) = Command::new(CLI_PATH).args(args).output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok(stdout);
        }
    }

    let output = Command::new("pkexec")
    .arg(CLI_PATH)
    .args(args)
    .output()
    .map_err(|e| format!("Не удалось запустить {CLI_PATH}: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        return Err(if !stderr.is_empty() { stderr } else { stdout });
    }

    Ok(stdout)
}

fn validate_network_id(id: &str) -> Result<(), String> {
    if id.len() != 16 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Network ID должен состоять из 16 шестнадцатеричных символов".into());
    }
    Ok(())
}

pub fn get_node_info() -> Result<NodeInfo, String> {
    let output = run_cli(&["-j", "info"])?;
    let json: Value = serde_json::from_str(&output)
    .map_err(|e| format!("Ошибка JSON info: {e}"))?;

    Ok(NodeInfo {
        address: json["address"].as_str().unwrap_or("—").to_string(),
       version: json["version"].as_str().unwrap_or("—").to_string(),
       online: json["online"].as_bool().unwrap_or(false),
    })
}

pub fn get_networks() -> Result<Vec<Network>, String> {
    let output = run_cli(&["-j", "listnetworks"])?;
    let json: Value = serde_json::from_str(&output)
    .map_err(|e| format!("Ошибка JSON networks: {e}"))?;

    let networks = json.as_array().ok_or("Некорректный формат")?;
    let mut result = Vec::new();

    for net in networks {
        let id = net["nwid"].as_str().unwrap_or("").to_string();
        let name = net["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or("Без имени")
        .to_string();
        let status = net["status"].as_str().unwrap_or("UNKNOWN").to_string();
        let mac = net["mac"].as_str().unwrap_or("—").to_string();

        // Корректно выбираем именно IPv4, если есть IPv6
        let ip = net["assignedAddresses"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
            .find_map(|v| v.as_str().filter(|s| s.contains('.')))
            .or_else(|| arr.first().and_then(Value::as_str))
        })
        .unwrap_or("Нет IP")
        .to_string();

        result.push(Network {
            id,
            name,
            status,
            ip,
            mac,
            allow_managed: net["allowManaged"].as_bool().unwrap_or(true),
                    allow_global: net["allowGlobal"].as_bool().unwrap_or(false),
                    allow_default: net["allowDefault"].as_bool().unwrap_or(false),
                    allow_dns: net["allowDNS"].as_bool().unwrap_or(false),
        });
    }

    Ok(result)
}

fn get_broadcast_ip(cidr: &str) -> Option<String> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let ip_str = parts[0];
    let prefix_len: u8 = parts[1].parse().ok()?;

    let octets: Vec<u8> = ip_str
    .split('.')
    .filter_map(|s| s.parse::<u8>().ok())
    .collect();

    if octets.len() != 4 || prefix_len > 32 {
        return None;
    }

    let ip_u32 = ((octets[0] as u32) << 24)
    | ((octets[1] as u32) << 16)
    | ((octets[2] as u32) << 8)
    | (octets[3] as u32);

    let mask = if prefix_len == 0 {
        0
    } else {
        !((1u32 << (32 - prefix_len)) - 1)
    };

    let bcast_u32 = ip_u32 | (!mask);

    Some(format!(
        "{}.{}.{}.{}",
        (bcast_u32 >> 24) & 0xff,
                 (bcast_u32 >> 16) & 0xff,
                 (bcast_u32 >> 8) & 0xff,
                 bcast_u32 & 0xff
    ))
}

fn ping_ip(ip: &str) -> (bool, i64) {
    // -n отключает лишний reverse DNS lookup, ускоряя опрос в разы
    if let Ok(output) = Command::new("ping")
        .args(["-c", "1", "-W", "1", "-n", ip])
        .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(pos) = stdout.find("time=") {
                    let rest = &stdout[pos + 5..];
                    if let Some(end) = rest.find(" ms") {
                        if let Ok(ms) = rest[..end].trim().parse::<f64>() {
                            return (true, ms.round() as i64);
                        }
                    }
                }
                return (true, 1);
            }
        }
        (false, -1)
}

pub fn get_network_members(networks: &[Network], do_broadcast: bool) -> Vec<Member> {
    let mut my_ips = Vec::new();

    for net in networks {
        if net.ip != "Нет IP" {
            let ip_only = net.ip.split('/').next().unwrap_or("").to_string();
            my_ips.push(ip_only);

            if do_broadcast {
                if let Some(bcast) = get_broadcast_ip(&net.ip) {
                    let _ = Command::new("ping")
                    .args(["-b", "-c", "1", "-W", "1", &bcast])
                    .output();
                }
            }
        }
    }

    let mut candidates = Vec::new();

    if let Ok(content) = fs::read_to_string("/proc/net/arp") {
        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 6 {
                let ip = parts[0].to_string();
                let flags = parts[2];
                let mac = parts[3].to_string();
                let dev = parts[5];

                if dev.starts_with("zt") && flags != "0x0" && !my_ips.contains(&ip) {
                    candidates.push((ip, mac));
                }
            }
        }
    }

    let mut handles = Vec::new();
    for (ip, mac) in candidates {
        handles.push(thread::spawn(move || {
            let (is_online, latency) = ping_ip(&ip);
            Member {
                ip,
                latency,
                is_online,
                mac,
            }
        }));
    }

    let mut members = Vec::new();
    for handle in handles {
        if let Ok(member) = handle.join() {
            members.push(member);
        }
    }

    members.sort_by(|a, b| b.is_online.cmp(&a.is_online));
    members
}

pub fn join_network(id: &str) -> Result<String, String> {
    let id = id.trim();
    validate_network_id(id)?;
    run_cli(&["join", id])
}

pub fn leave_network(id: &str) -> Result<String, String> {
    let id = id.trim();
    validate_network_id(id)?;
    run_cli(&["leave", id])
}

pub fn set_network_setting(id: &str, setting: &str, value: bool) -> Result<String, String> {
    let id = id.trim();
    validate_network_id(id)?;
    let val_str = if value { "1" } else { "0" };
    run_cli(&["set", id, &format!("{setting}={val_str}")])
}
