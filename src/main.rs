//! Яндекс Музыка RS — неофициальный клиент Яндекс Музыки. Точка входа: один экземпляр
//! на пользователя, выбор рендерера, запуск окна.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ahead;
mod api;
mod assets;
mod auth;
mod client;
mod config;
mod discord;
mod eq;
mod flac;
mod gui;
mod history;
mod i18n;
mod images;
mod landing;
#[cfg(target_os = "linux")]
mod mpris;
mod net;
mod palette;
mod player;
mod save;
mod secret;
mod stream;
#[cfg_attr(windows, path = "sys/windows.rs")]
#[cfg_attr(target_os = "linux", path = "sys/linux.rs")]
mod sys;
mod tags;
#[cfg(windows)]
mod tray;
mod update;
mod vibe;
mod yandex_id;

/// Окно проверочного запуска: за краем экрана, без фокуса и без кнопки на панели задач.
fn offscreen(attrs: slint::winit_030::winit::window::WindowAttributes) -> slint::winit_030::winit::window::WindowAttributes {
    use slint::winit_030::winit;
    let attrs = attrs.with_active(false).with_position(winit::dpi::PhysicalPosition::new(-20000, -20000));
    #[cfg(windows)]
    let attrs = winit::platform::windows::WindowAttributesExtWindows::with_skip_taskbar(attrs, true);
    attrs
}

fn main() -> Result<(), slint::PlatformError> {
    #[cfg(target_os = "linux")]
    sys::audio_identity();
    // После обновления старая версия ещё закрывается: ждём её до 15 секунд.
    let after_update = std::env::args().any(|a| a == update::AFTER_UPDATE);
    let sandbox = config::sandbox();
    let mut claimed = sandbox || sys::claim_instance();
    for _ in 0..60 {
        if claimed || !after_update {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        claimed = sys::claim_instance();
    }
    if !claimed {
        // Второй запуск просто показывает уже открытое окно.
        sys::wake_existing();
        return Ok(());
    }
    // Окно рисует Skia. Без видеокарты память не зависит от драйвера (с D3D или OpenGL окно
    // занимает в 2–3 раза больше), но пятно «Моей волны» на большом экране процессор не
    // тянет: на 3000×1700 это больше ядра и пропуски кадров. Поэтому в «Плавном» режиме рисует
    // видеокарта (настройка «Отрисовка видеокартой»); если её нет, Slint сам переходит на
    // процессор. Режим работы меняет анимации и кэши на лету (gui::apply_mode), рендерер —
    // после перезапуска. YAMUSIC_RS_RENDERER задаёт рендерер напрямую — для проверки.
    let renderer = std::env::var("YAMUSIC_RS_RENDERER").unwrap_or_else(|_| if config::gpu_wanted() { "skia" } else { "skia-software" }.into());
    config::GPU.store(renderer != "skia-software", std::sync::atomic::Ordering::Relaxed);
    let mut backend = slint::BackendSelector::new().backend_name("winit".into()).renderer_name(renderer);
    if sandbox {
        backend = backend.with_winit_window_attributes_hook(offscreen);
    }
    backend.select()?;
    // По app_id (Wayland) и WM_CLASS (X11) окружение находит .desktop и значок.
    #[cfg(target_os = "linux")]
    slint::set_xdg_app_id("yamusic-rs")?;
    update::cleanup();
    if !sandbox {
        sys::refresh_autostart();
    }
    // Без трея на Linux спрятанное окно было бы не вернуть.
    let start_hidden = cfg!(windows) && std::env::args().any(|a| a == "--tray");
    gui::run(start_hidden)
}
