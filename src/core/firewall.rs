use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub enum FirewallType {
    Ufw,
    Firewalld,
    None,
}

#[derive(Debug, Clone)]
pub struct FirewallStatus {
    pub fw_type: FirewallType,
    pub is_active: bool,
    pub title: String,
}

pub fn get_status() -> FirewallStatus {
    let ufw_exists = Path::new("/usr/bin/ufw").exists();
    let fw_exists = Path::new("/usr/bin/firewall-cmd").exists();

    if ufw_exists {
        let is_active = Command::new("systemctl")
        .args(["is-active", "--quiet", "ufw.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

        if is_active {
            return FirewallStatus {
                fw_type: FirewallType::Ufw,
                is_active: true,
                title: "UFW активен (блокирует входящие)".into(),
            };
        }
    }

    if fw_exists {
        let is_active = Command::new("systemctl")
        .args(["is-active", "--quiet", "firewalld.service"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

        if is_active {
            return FirewallStatus {
                fw_type: FirewallType::Firewalld,
                is_active: true,
                title: "Firewalld активен (блокирует входящие)".into(),
            };
        }
    }

    if ufw_exists {
        return FirewallStatus {
            fw_type: FirewallType::Ufw,
            is_active: false,
            title: "UFW отключен".into(),
        };
    }

    if fw_exists {
        return FirewallStatus {
            fw_type: FirewallType::Firewalld,
            is_active: false,
            title: "Firewalld остановлен".into(),
        };
    }

    FirewallStatus {
        fw_type: FirewallType::None,
        is_active: false,
        title: "Файрвол не обнаружен (трафик открыт)".into(),
    }
}

pub fn toggle_firewall(fw_type: FirewallType, enable: bool) -> Result<String, String> {
    match fw_type {
        FirewallType::Ufw => {
            let cmd = if enable { "enable" } else { "disable" };
            let output = Command::new("pkexec")
            .args(["ufw", cmd])
            .output()
            .map_err(|e| format!("Ошибка pkexec: {e}"))?;

            if !output.status.success() {
                return Err("Не удалось изменить статус UFW".into());
            }

            Ok(if enable { "UFW включен" } else { "UFW отключен" }.into())
        }
        FirewallType::Firewalld => {
            let action = if enable { "start" } else { "stop" };
            let output = Command::new("pkexec")
            .args(["systemctl", action, "firewalld.service"])
            .output()
            .map_err(|e| format!("Ошибка pkexec: {e}"))?;

            if !output.status.success() {
                return Err("Не удалось изменить статус Firewalld".into());
            }

            Ok(if enable {
                "Firewalld запущен"
            } else {
                "Firewalld остановлен"
            }
            .into())
        }
        FirewallType::None => Err("Файрвол не установлен в системе".into()),
    }
}

pub fn allow_zerotier_interfaces(fw_type: FirewallType) -> Result<String, String> {
    match fw_type {
        FirewallType::Ufw => {
            let output = Command::new("pkexec")
            .args(["ufw", "allow", "in", "on", "zt+", "to", "any"])
            .output()
            .map_err(|e| format!("Ошибка pkexec: {e}"))?;

            if !output.status.success() {
                return Err("Не удалось добавить правило в UFW".into());
            }

            Ok("Трафик ZeroTier (zt+) разрешён в UFW".into())
        }
        FirewallType::Firewalld => {
            // Игнорируем код ошибки, если интерфейс уже был добавлен (ALREADY_ENABLED)
            let script = "firewall-cmd --zone=trusted --add-interface=zt+ --permanent 2>&1; firewall-cmd --reload";
            let output = Command::new("pkexec")
            .args(["sh", "-c", script])
            .output()
            .map_err(|e| format!("Ошибка pkexec: {e}"))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            if output.status.success() || stdout.contains("ALREADY_ENABLED") {
                Ok("Интерфейсы zt+ разрешены в Firewalld".into())
            } else {
                Err("Не удалось применить настройки в Firewalld".into())
            }
        }
        FirewallType::None => Err("Файрвол не установлен".into()),
    }
}
