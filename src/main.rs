use adw::prelude::*;
use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

mod core;
use core::firewall::{self, FirewallStatus, FirewallType};
use core::zerotier::{self, Member, Network, NodeInfo};

struct NetworkRowWidgets {
    card: adw::ExpanderRow,
    status_badge: gtk::Label,
    mac_row: adw::ActionRow,
    sw_managed: adw::SwitchRow,
    sw_global: adw::SwitchRow,
    sw_default: adw::SwitchRow,
    sw_dns: adw::SwitchRow,
    is_updating: Rc<RefCell<bool>>,
}

struct MemberRowWidgets {
    row: adw::ActionRow,
    badge: gtk::Label,
}

#[derive(Clone)]
struct UiState {
    window: adw::ApplicationWindow,
    node_row: adw::ActionRow,
    status_row: adw::ActionRow,
    service_btn: gtk::Button,
    autostart_row: adw::SwitchRow,
    networks_group: adw::PreferencesGroup,
    empty_status: adw::StatusPage,
    members_group: adw::PreferencesGroup,
    empty_members: adw::StatusPage,
    firewall_row: adw::ActionRow,
    firewall_btn: gtk::Button,
    allow_zt_btn: gtk::Button,
    spinner: gtk::Spinner,
    overlay: adw::ToastOverlay,
    banner: adw::Banner,
    active_networks: Rc<RefCell<HashMap<String, NetworkRowWidgets>>>,
    active_members: Rc<RefCell<HashMap<String, MemberRowWidgets>>>,
    is_loading_autostart: Rc<RefCell<bool>>,
    is_reloading: Rc<RefCell<bool>>,
    broadcast_ticks: Rc<RefCell<u32>>,
}

fn show_toast(overlay: &adw::ToastOverlay, text: &str) {
    let toast = adw::Toast::new(text);
    toast.set_timeout(3);
    overlay.add_toast(toast);
}

// 100% неблокирующее выполнение: фоновый поток + 60fps опрос через try_recv
fn async_action<T: Send + 'static, F: FnOnce() -> T + Send + 'static, C: FnOnce(T) + 'static>(
    action: F,
    callback: C,
) {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let res = action();
        let _ = sender.send(res);
    });

    let mut callback_opt = Some(callback);
    glib::timeout_add_local(Duration::from_millis(16), move || {
        match receiver.try_recv() {
            Ok(res) => {
                if let Some(cb) = callback_opt.take() {
                    cb(res);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn build_network_card(
    network: &Network,
    window: &adw::ApplicationWindow,
    overlay: &adw::ToastOverlay,
    on_refresh: impl Fn() + 'static + Clone,
) -> NetworkRowWidgets {
    let row = adw::ExpanderRow::new();
    row.set_title(&network.name);
    row.set_subtitle(&format!("ID: {} • IP: {}", network.id, network.ip));

    let status_badge = gtk::Label::new(None);
    status_badge.set_valign(gtk::Align::Center);
    update_network_status_badge(&status_badge, &network.status);
    status_badge.add_css_class("caption");
    row.add_suffix(&status_badge);

    let copy_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    copy_box.set_valign(gtk::Align::Center);

    let copy_ip = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy_ip.set_tooltip_text(Some("Скопировать чистый IP (без маски)"));
    copy_ip.add_css_class("flat");
    let ip_str = network.ip.clone();
    let toast_ip = overlay.clone();
    copy_ip.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            // Копируем чистый IP-адрес без /24
            let clean_ip = ip_str.split('/').next().unwrap_or(&ip_str).to_string();
            display.clipboard().set_text(&clean_ip);
            show_toast(&toast_ip, &format!("IP скопирован: {clean_ip}"));
        }
    });

    let copy_id = gtk::Button::from_icon_name("accessories-dictionary-symbolic");
    copy_id.set_tooltip_text(Some("Скопировать Network ID"));
    copy_id.add_css_class("flat");
    let id_str = network.id.clone();
    let toast_id = overlay.clone();
    copy_id.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&id_str);
            show_toast(&toast_id, "Network ID скопирован");
        }
    });

    copy_box.append(&copy_ip);
    copy_box.append(&copy_id);
    row.add_suffix(&copy_box);

    let mac_row = adw::ActionRow::new();
    mac_row.set_title("MAC-адрес интерфейса");
    mac_row.set_subtitle(&network.mac);
    row.add_row(&mac_row);

    let is_updating = Rc::new(RefCell::new(false));

    let make_switch = |title: &str, subtitle: &str, current: bool, setting: &'static str| {
        let sw_row = adw::SwitchRow::new();
        sw_row.set_title(title);
        sw_row.set_subtitle(subtitle);
        sw_row.set_active(current);

        let net_id = network.id.clone();
        let toast_sw = overlay.clone();
        let is_up_c = is_updating.clone();
        let sw_c = sw_row.clone();

        sw_row.connect_active_notify(move |sw| {
            if *is_up_c.borrow() {
                return;
            }
            let val = sw.is_active();
            let net_id_c = net_id.clone();
            let toast_c = toast_sw.clone();
            let sw_ref = sw_c.clone();
            let is_up_rollback = is_up_c.clone();

            async_action(
                move || zerotier::set_network_setting(&net_id_c, setting, val),
                         move |res| match res {
                             Ok(_) => show_toast(&toast_c, &format!("Параметр {setting} обновлён")),
                         Err(e) => {
                             show_toast(&toast_c, &format!("Ошибка: {e}"));
                             *is_up_rollback.borrow_mut() = true;
                             sw_ref.set_active(!val);
                             *is_up_rollback.borrow_mut() = false;
                         }
                         },
            );
        });
        sw_row
    };

    let sw_managed = make_switch(
        "Allow Managed",
        "Разрешить назначение приватных IP-адресов",
        network.allow_managed,
        "allowManaged",
    );
    let sw_global = make_switch(
        "Allow Global",
        "Разрешить глобальные публичные IP-адреса",
        network.allow_global,
        "allowGlobal",
    );
    let sw_default = make_switch(
        "Allow Default Route",
        "Использовать сеть как шлюз по умолчанию",
        network.allow_default,
        "allowDefault",
    );
    let sw_dns = make_switch(
        "Allow DNS",
        "Принимать DNS-серверы от сети",
        network.allow_dns,
        "allowDNS",
    );

    row.add_row(&sw_managed);
    row.add_row(&sw_global);
    row.add_row(&sw_default);
    row.add_row(&sw_dns);

    let leave_row = adw::ActionRow::new();
    let leave_btn = gtk::Button::with_label("Покинуть сеть");
    leave_btn.add_css_class("destructive-action");
    leave_btn.set_valign(gtk::Align::Center);

    let net_id_leave = network.id.clone();
    let net_name_leave = network.name.clone();
    let toast_leave = overlay.clone();
    let refresh_cb = on_refresh;
    let win_clone = window.clone();

    leave_btn.connect_clicked(move |_| {
        let net_id = net_id_leave.clone();
        let toast = toast_leave.clone();
        let refresh = refresh_cb.clone();

        let dialog = adw::AlertDialog::new(
            Some("Отключиться от сети?"),
                                           Some(&format!(
                                               "Вы уверены, что хотите покинуть сеть «{}» ({})?",
                                                         net_name_leave, net_id
                                           )),
        );
        dialog.add_response("cancel", "Отмена");
        dialog.add_response("leave", "Отключиться");
        dialog.set_response_appearance("leave", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));

        dialog.choose(Some(&win_clone), None::<&gio::Cancellable>, move |response| {
            if response == "leave" {
                async_action(
                    move || zerotier::leave_network(&net_id),
                             move |res| match res {
                                 Ok(_) => {
                                     show_toast(&toast, "Сеть отключена");
                                     refresh();
                                 }
                                 Err(e) => show_toast(&toast, &format!("Ошибка: {e}")),
                             },
                );
            }
        });
    });

    leave_row.add_suffix(&leave_btn);
    row.add_row(&leave_row);

    NetworkRowWidgets {
        card: row,
        status_badge,
        mac_row,
        sw_managed,
        sw_global,
        sw_default,
        sw_dns,
        is_updating,
    }
}

fn update_network_status_badge(badge: &gtk::Label, status: &str) {
    badge.remove_css_class("success");
    badge.remove_css_class("warning");
    badge.remove_css_class("error");
    badge.remove_css_class("dim-label");

    match status {
        "OK" => {
            badge.set_text("OK");
            badge.add_css_class("success");
        }
        "ACCESS_DENIED" => {
            badge.set_text("Нет доступа (авторизуйте в панели)");
            badge.add_css_class("error");
        }
        "REQUESTING_CONFIGURATION" => {
            badge.set_text("Настройка...");
            badge.add_css_class("warning");
        }
        "NOT_FOUND" => {
            badge.set_text("Не найдена");
            badge.add_css_class("error");
        }
        other => {
            badge.set_text(other);
            badge.add_css_class("dim-label");
        }
    }
}

fn build_member_row(member: &Member, overlay: &adw::ToastOverlay) -> MemberRowWidgets {
    let row = adw::ActionRow::new();
    row.set_title(&member.ip);

    let (status_text, ping_text) = if member.is_online {
        ("В сети", format!("{} ms", member.latency))
    } else {
        ("Не отвечает", "—".into())
    };

    row.set_subtitle(&format!("{status_text} • Пинг: {ping_text} • MAC: {}", member.mac));

    let badge = gtk::Label::new(Some(if member.is_online { "ONLINE" } else { "OFFLINE" }));
    badge.set_valign(gtk::Align::Center);
    badge.add_css_class("caption");
    if member.is_online {
        badge.add_css_class("success");
    } else {
        badge.add_css_class("dim-label");
    }
    row.add_suffix(&badge);

    let copy_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy_btn.set_tooltip_text(Some("Скопировать ZeroTier IP"));
    copy_btn.set_valign(gtk::Align::Center);
    copy_btn.add_css_class("flat");

    let ip_str = member.ip.clone();
    let toast = overlay.clone();
    copy_btn.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&ip_str);
            show_toast(&toast, "ZeroTier IP скопирован");
        }
    });

    row.add_suffix(&copy_btn);
    MemberRowWidgets { row, badge }
}

struct FetchResult {
    is_active: bool,
    is_enabled: bool,
    node_info: Result<NodeInfo, String>,
    networks: Result<Vec<Network>, String>,
    members: Vec<Member>,
    fw_status: FirewallStatus,
}

fn reload_data(state: UiState, force_broadcast: bool) {
    // Предотвращаем наложение запросов при долгом ответе
    if *state.is_reloading.borrow() {
        return;
    }
    *state.is_reloading.borrow_mut() = true;

    state.spinner.start();
    let state_c = state.clone();

    state.banner.set_revealed(!zerotier::has_auth_token());

    let do_broadcast = if force_broadcast {
        *state.broadcast_ticks.borrow_mut() = 0;
        true
    } else {
        let mut t = state.broadcast_ticks.borrow_mut();
        *t += 1;
        if *t >= 6 {
            *t = 0;
            true
        } else {
            false
        }
    };

    async_action(
        move || {
            let fw_status = firewall::get_status();
            let is_enabled = zerotier::is_service_enabled();
            let active = zerotier::is_service_active();

            if !active {
                return FetchResult {
                    is_active: false,
                    is_enabled,
                    node_info: Err("Служба остановлена".into()),
                 networks: Ok(Vec::new()),
                 members: Vec::new(),
                 fw_status,
                };
            }

            let node_info = zerotier::get_node_info();
            let networks = zerotier::get_networks();

            let members = if let Ok(ref nets) = networks {
                zerotier::get_network_members(nets, do_broadcast)
            } else {
                Vec::new()
            };

            FetchResult {
                is_active: true,
                is_enabled,
                node_info,
                networks,
                members,
                fw_status,
            }
        },
        move |result| {
            state_c.spinner.stop();
            *state_c.is_reloading.borrow_mut() = false;

            // 1. Автозапуск и служба
            *state_c.is_loading_autostart.borrow_mut() = true;
            state_c.autostart_row.set_active(result.is_enabled);
            *state_c.is_loading_autostart.borrow_mut() = false;

            if result.is_active {
                state_c.service_btn.set_label("Остановить");
                state_c.service_btn.remove_css_class("suggested-action");
                state_c.service_btn.add_css_class("destructive-action");

                match result.node_info {
                    Ok(info) => {
                        state_c.node_row.set_subtitle(&info.address);
                        let st = if info.online {
                            "ONLINE (готов к работе)"
                        } else {
                            "OFFLINE (подключение к контроллерам...)"
                        };
                        state_c.status_row.set_subtitle(&format!("{st} • Версия v{}", info.version));
                    }
                    Err(_) => {
                        state_c.node_row.set_subtitle("—");
                        state_c.status_row.set_subtitle("Служба активна, ожидание ответа...");
                    }
                }
            } else {
                state_c.service_btn.set_label("Запустить");
                state_c.service_btn.remove_css_class("destructive-action");
                state_c.service_btn.add_css_class("suggested-action");
                state_c.node_row.set_subtitle("—");
                state_c.status_row.set_subtitle("Служба zerotier-one остановлена");
            }

            // 2. Файрвол
            state_c.firewall_row.set_subtitle(&result.fw_status.title);
            if result.fw_status.fw_type == FirewallType::None {
                state_c.firewall_btn.set_visible(false);
                state_c.allow_zt_btn.set_visible(false);
            } else {
                state_c.firewall_btn.set_visible(true);
                state_c.allow_zt_btn.set_visible(result.fw_status.is_active);

                if result.fw_status.is_active {
                    state_c.firewall_btn.set_label("Отключить");
                    state_c.firewall_btn.remove_css_class("suggested-action");
                    state_c.firewall_btn.add_css_class("destructive-action");
                } else {
                    state_c.firewall_btn.set_label("Включить");
                    state_c.firewall_btn.remove_css_class("destructive-action");
                    state_c.firewall_btn.add_css_class("suggested-action");
                }
            }

            // 3. Сети
            let mut current_networks = state_c.active_networks.borrow_mut();

            if !result.is_active {
                for (_, widgets) in current_networks.drain() {
                    state_c.networks_group.remove(&widgets.card);
                }
                state_c.empty_status.set_title("Служба остановлена");
                state_c.empty_status.set_description(Some("Включите службу для просмотра сетей"));
                state_c.empty_status.set_visible(true);
                state_c.networks_group.set_visible(false);
            } else if let Ok(networks) = result.networks {
                if networks.is_empty() {
                    for (_, widgets) in current_networks.drain() {
                        state_c.networks_group.remove(&widgets.card);
                    }
                    state_c.empty_status.set_title("Нет активных сетей");
                    state_c.empty_status.set_description(Some("Подключитесь к первой сети выше"));
                    state_c.empty_status.set_visible(true);
                    state_c.networks_group.set_visible(false);
                } else {
                    state_c.empty_status.set_visible(false);
                    state_c.networks_group.set_visible(true);

                    current_networks.retain(|id, widgets| {
                        let exists = networks.iter().any(|n| &n.id == id);
                        if !exists {
                            state_c.networks_group.remove(&widgets.card);
                        }
                        exists
                    });

                    for net in &networks {
                        if let Some(widgets) = current_networks.get_mut(&net.id) {
                            widgets.card.set_title(&net.name);
                            widgets.card.set_subtitle(&format!("ID: {} • IP: {}", net.id, net.ip));
                            update_network_status_badge(&widgets.status_badge, &net.status);
                            widgets.mac_row.set_subtitle(&net.mac);

                            *widgets.is_updating.borrow_mut() = true;
                            widgets.sw_managed.set_active(net.allow_managed);
                            widgets.sw_global.set_active(net.allow_global);
                            widgets.sw_default.set_active(net.allow_default);
                            widgets.sw_dns.set_active(net.allow_dns);
                            *widgets.is_updating.borrow_mut() = false;
                        } else {
                            let st = state_c.clone();
                            let widgets = build_network_card(
                                net,
                                &state_c.window,
                                &state_c.overlay,
                                move || reload_data(st.clone(), true),
                            );
                            state_c.networks_group.add(&widgets.card);
                            current_networks.insert(net.id.clone(), widgets);
                        }
                    }
                }
            }

            // 4. Участники
            let mut current_members = state_c.active_members.borrow_mut();

            if !result.is_active {
                for (_, widgets) in current_members.drain() {
                    state_c.members_group.remove(&widgets.row);
                }
                state_c.empty_members.set_title("Служба остановлена");
                state_c.empty_members.set_description(Some("Запустите службу ZeroTier для поиска участников"));
                state_c.empty_members.set_visible(true);
                state_c.members_group.set_visible(false);
            } else if result.members.is_empty() {
                for (_, widgets) in current_members.drain() {
                    state_c.members_group.remove(&widgets.row);
                }
                state_c.empty_members.set_title("Участники пока не обнаружены");
                state_c.empty_members.set_description(Some("Когда другие устройства подключатся к сети, их ZeroTier IP отобразятся здесь"));
                state_c.empty_members.set_visible(true);
                state_c.members_group.set_visible(false);
            } else {
                state_c.empty_members.set_visible(false);
                state_c.members_group.set_visible(true);

                current_members.retain(|ip, widgets| {
                    let exists = result.members.iter().any(|m| &m.ip == ip);
                    if !exists {
                        state_c.members_group.remove(&widgets.row);
                    }
                    exists
                });

                for member in &result.members {
                    let (status_text, ping_text) = if member.is_online {
                        ("В сети", format!("{} ms", member.latency))
                    } else {
                        ("Не отвечает", "—".into())
                    };

                    if let Some(widgets) = current_members.get_mut(&member.ip) {
                        widgets.row.set_subtitle(&format!(
                            "{status_text} • Пинг: {ping_text} • MAC: {}",
                            member.mac
                        ));
                        widgets.badge.set_text(if member.is_online { "ONLINE" } else { "OFFLINE" });
                        if member.is_online {
                            widgets.badge.remove_css_class("dim-label");
                            widgets.badge.add_css_class("success");
                        } else {
                            widgets.badge.remove_css_class("success");
                            widgets.badge.add_css_class("dim-label");
                        }
                    } else {
                        let widgets = build_member_row(member, &state_c.overlay);
                        state_c.members_group.add(&widgets.row);
                        current_members.insert(member.ip.clone(), widgets);
                    }
                }
            }
        },
    );
}

fn show_about_dialog(window: &adw::ApplicationWindow) {
    let dialog = adw::AboutDialog::builder()
    .application_name("ZeroTier Manager")
    .application_icon("network-vpn-symbolic")
    .developer_name("Arch Linux Community")
    .version("0.2.2")
    .copyright("© 2026")
    .license_type(gtk::License::MitX11)
    .website("https://zerotier.com")
    .issue_url("https://github.com")
    .comments("Быстрый нативный интерфейс ZeroTier для Arch Linux на базе GTK4 и Libadwaita.")
    .build();

    dialog.present(Some(window));
}

fn build_ui(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
    .application(app)
    .title("ZeroTier")
    .default_width(750)
    .default_height(720)
    .build();

    let toast_overlay = adw::ToastOverlay::new();
    let toolbar_view = adw::ToolbarView::new();
    let header_bar = adw::HeaderBar::new();

    let spinner = gtk::Spinner::new();
    header_bar.pack_start(&spinner);

    let view_stack = adw::ViewStack::new();
    let view_switcher = adw::ViewSwitcher::builder()
    .stack(&view_stack)
    .policy(adw::ViewSwitcherPolicy::Wide)
    .build();
    header_bar.set_title_widget(Some(&view_switcher));

    let menu_model = gio::Menu::new();
    menu_model.append(Some("Обновить (Ctrl+R)"), Some("app.refresh"));
    menu_model.append(Some("О программе"), Some("app.about"));

    let menu_btn = gtk::MenuButton::builder()
    .icon_name("open-menu-symbolic")
    .tooltip_text("Главное меню")
    .menu_model(&menu_model)
    .build();

    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Обновить (Ctrl+R)"));

    header_bar.pack_end(&menu_btn);
    header_bar.pack_end(&refresh_btn);

    toolbar_view.add_top_bar(&header_bar);

    let banner = adw::Banner::new("Для работы без запросов пароля настройте локальный токен.");
    banner.set_button_label(Some("Настроить доступ"));
    banner.set_revealed(false);
    toolbar_view.add_top_bar(&banner);

    // --- ВКЛАДКА 1: СЕТИ ---
    let page_networks = adw::PreferencesPage::new();

    let node_group = adw::PreferencesGroup::new();
    node_group.set_title("Локальный узел");

    let node_row = adw::ActionRow::new();
    node_row.set_title("Node ID (Адрес узла)");
    node_row.set_subtitle("—");

    let copy_node_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy_node_btn.set_valign(gtk::Align::Center);
    copy_node_btn.add_css_class("flat");
    let nr_clone = node_row.clone();
    let toast_node = toast_overlay.clone();
    copy_node_btn.connect_clicked(move |_| {
        if let Some(sub) = nr_clone.subtitle() {
            if sub != "—" {
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&sub);
                    show_toast(&toast_node, "Node ID скопирован");
                }
            }
        }
    });
    node_row.add_suffix(&copy_node_btn);

    let status_row = adw::ActionRow::new();
    status_row.set_title("Служба zerotier-one");
    status_row.set_subtitle("Проверка...");

    let service_btn = gtk::Button::with_label("Запустить");
    service_btn.set_valign(gtk::Align::Center);
    status_row.add_suffix(&service_btn);

    let autostart_row = adw::SwitchRow::new();
    autostart_row.set_title("Автозапуск службы");
    autostart_row.set_subtitle("Запускать демон zerotier-one при старте системы");

    node_group.add(&node_row);
    node_group.add(&status_row);
    node_group.add(&autostart_row);
    page_networks.add(&node_group);

    let fw_group = adw::PreferencesGroup::new();
    fw_group.set_title("Сетевой экран (Файрвол)");
    fw_group.set_description(Some("Блокировка входящих подключений (игр, SSH, общих папок)"));

    let firewall_row = adw::ActionRow::new();
    firewall_row.set_title("Состояние файрвола");
    firewall_row.set_subtitle("Определение...");

    let fw_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    fw_box.set_valign(gtk::Align::Center);

    let allow_zt_btn = gtk::Button::with_label("Разрешить zt*");
    allow_zt_btn.set_tooltip_text(Some("Разрешить трафик ZeroTier без отключения файрвола"));
    allow_zt_btn.add_css_class("flat");

    let firewall_btn = gtk::Button::with_label("Изменить");
    firewall_btn.set_valign(gtk::Align::Center);

    fw_box.append(&allow_zt_btn);
    fw_box.append(&firewall_btn);
    firewall_row.add_suffix(&fw_box);

    fw_group.add(&firewall_row);
    page_networks.add(&fw_group);

    let join_group = adw::PreferencesGroup::new();
    join_group.set_title("Подключение к сети");
    join_group.set_description(Some("Введите 16-значный идентификатор сети (Network ID)"));

    let join_entry_row = adw::EntryRow::new();
    join_entry_row.set_title("Network ID");
    join_entry_row.set_show_apply_button(true);

    join_entry_row.connect_changed(|entry| {
        let text = entry.text();
        let valid = text.len() == 16 && text.chars().all(|c| c.is_ascii_hexdigit());
        if text.is_empty() || valid {
            entry.remove_css_class("error");
        } else {
            entry.add_css_class("error");
        }
    });

    join_group.add(&join_entry_row);
    page_networks.add(&join_group);

    let networks_group = adw::PreferencesGroup::new();
    networks_group.set_title("Подключенные сети");

    let empty_status = adw::StatusPage::new();
    empty_status.set_icon_name(Some("network-vpn-symbolic"));
    empty_status.set_title("Нет активных сетей");
    empty_status.set_description(Some("Подключитесь к первой сети с помощью поля выше"));
    empty_status.set_visible(false);

    page_networks.add(&networks_group);

    let net_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    net_box.append(&page_networks);
    net_box.append(&empty_status);

    view_stack.add_titled_with_icon(&net_box, Some("networks"), "Сети", "network-workgroup-symbolic");

    // --- ВКЛАДКА 2: УЧАСТНИКИ СЕТИ ---
    let page_members = adw::PreferencesPage::new();

    let members_group = adw::PreferencesGroup::new();
    members_group.set_title("Участники в виртуальной сети");
    members_group.set_description(Some("Локальные ZeroTier IP-адреса других участников сети"));

    let empty_members = adw::StatusPage::new();
    empty_members.set_icon_name(Some("system-users-symbolic"));
    empty_members.set_title("Участники пока не обнаружены");
    empty_members.set_description(Some("Когда другие устройства подключатся к сети, их ZeroTier IP отобразятся здесь"));

    page_members.add(&members_group);

    let members_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    members_box.append(&page_members);
    members_box.append(&empty_members);

    view_stack.add_titled_with_icon(&members_box, Some("members"), "Участники", "system-users-symbolic");

    toolbar_view.set_content(Some(&view_stack));
    toast_overlay.set_child(Some(&toolbar_view));
    window.set_content(Some(&toast_overlay));

    let state = UiState {
        window: window.clone(),
        node_row,
        status_row,
        service_btn: service_btn.clone(),
        autostart_row: autostart_row.clone(),
        networks_group,
        empty_status,
        members_group,
        empty_members,
        firewall_row,
        firewall_btn: firewall_btn.clone(),
        allow_zt_btn: allow_zt_btn.clone(),
        spinner,
        overlay: toast_overlay.clone(),
        banner: banner.clone(),
        active_networks: Rc::new(RefCell::new(HashMap::new())),
        active_members: Rc::new(RefCell::new(HashMap::new())),
        is_loading_autostart: Rc::new(RefCell::new(false)),
        is_reloading: Rc::new(RefCell::new(false)),
        broadcast_ticks: Rc::new(RefCell::new(0)),
    };

    let state_auto = state.clone();
    autostart_row.connect_active_notify(move |sw| {
        if *state_auto.is_loading_autostart.borrow() {
            return;
        }
        let enable = sw.is_active();
        let toast = state_auto.overlay.clone();
        let sw_c = sw.clone();
        let is_loading_c = state_auto.is_loading_autostart.clone();

        async_action(
            move || zerotier::set_service_enabled(enable),
                     move |res| match res {
                         Ok(msg) => show_toast(&toast, &msg),
                     Err(e) => {
                         show_toast(&toast, &format!("Ошибка: {e}"));
                         *is_loading_c.borrow_mut() = true;
                         sw_c.set_active(!enable);
                         *is_loading_c.borrow_mut() = false;
                     }
                     },
        );
    });

    let state_banner = state.clone();
    banner.connect_button_clicked(move |_| {
        let st = state_banner.clone();
        async_action(
            zerotier::setup_auth_token,
            move |res| match res {
                Ok(msg) => {
                    show_toast(&st.overlay, &msg);
                    reload_data(st, true);
                }
                Err(e) => show_toast(&st.overlay, &format!("Ошибка: {e}")),
            },
        );
    });

    let state_fw = state.clone();
    firewall_btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        let btn_c = btn.clone();
        let st = state_fw.clone();

        async_action(
            move || {
                let status = firewall::get_status();
                firewall::toggle_firewall(status.fw_type, !status.is_active)
            },
            move |res| {
                btn_c.set_sensitive(true);
                match res {
                    Ok(msg) => show_toast(&st.overlay, &msg),
                     Err(e) => show_toast(&st.overlay, &format!("Ошибка: {e}")),
                }
                reload_data(st, false);
            },
        );
    });

    let state_allow = state.clone();
    allow_zt_btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        let btn_c = btn.clone();
        let st = state_allow.clone();

        async_action(
            move || {
                let status = firewall::get_status();
                firewall::allow_zerotier_interfaces(status.fw_type)
            },
            move |res| {
                btn_c.set_sensitive(true);
                match res {
                    Ok(msg) => show_toast(&st.overlay, &msg),
                     Err(e) => show_toast(&st.overlay, &format!("Ошибка: {e}")),
                }
                reload_data(st, false);
            },
        );
    });

    let state_service = state.clone();
    service_btn.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        let btn_c = btn.clone();
        let st = state_service.clone();

        async_action(
            move || {
                if zerotier::is_service_active() {
                    zerotier::stop_service()
                } else {
                    zerotier::start_service()
                }
            },
            move |res| {
                btn_c.set_sensitive(true);
                match res {
                    Ok(msg) => show_toast(&st.overlay, &msg),
                     Err(e) => show_toast(&st.overlay, &format!("Ошибка: {e}")),
                }
                reload_data(st, true);
            },
        );
    });

    let entry_c = join_entry_row.clone();
    let state_join = state.clone();
    join_entry_row.connect_apply(move |_| {
        let net_id = entry_c.text().to_string();
        if net_id.len() != 16 || !net_id.chars().all(|c| c.is_ascii_hexdigit()) {
            show_toast(&state_join.overlay, "Network ID должен состоять из 16 шестнадцатеричных символов");
            return;
        }

        let entry_clean = entry_c.clone();
        let st = state_join.clone();

        async_action(
            move || zerotier::join_network(&net_id),
                     move |res| match res {
                         Ok(msg) => {
                             entry_clean.set_text("");
                             show_toast(&st.overlay, &format!("Успешно: {msg}"));
                             reload_data(st, true);
                         }
                         Err(e) => show_toast(&st.overlay, &format!("Ошибка подключения: {e}")),
                     },
        );
    });

    let state_refresh = state.clone();
    refresh_btn.connect_clicked(move |_| {
        reload_data(state_refresh.clone(), true);
    });

    let action_refresh = gio::SimpleAction::new("refresh", None);
    let state_action = state.clone();
    action_refresh.connect_activate(move |_, _| {
        reload_data(state_action.clone(), true);
    });
    app.add_action(&action_refresh);
    app.set_accels_for_action("app.refresh", &["<Control>r"]);

    let action_about = gio::SimpleAction::new("about", None);
    let win_for_about = window.clone();
    action_about.connect_activate(move |_, _| {
        show_about_dialog(&win_for_about);
    });
    app.add_action(&action_about);

    let state_timer = state.clone();
    glib::timeout_add_seconds_local(5, move || {
        reload_data(state_timer.clone(), false);
        glib::ControlFlow::Continue
    });

    reload_data(state, true);
    window.present();
}

fn main() {
    let app = adw::Application::builder()
    .application_id("org.archlinux.zerotier_manager")
    .build();

    app.connect_activate(build_ui);
    app.run();
}
