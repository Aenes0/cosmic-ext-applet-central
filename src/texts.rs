// SPDX-License-Identifier: GPL-3.0-only

//! The fork's own words, apart from `i18n.rs` to keep merges from upstream
//! simple. Portuguese or English, as `i18n` picks.

pub struct Texts {
    pub always: &'static str,
    pub when_active: &'static str,
    pub never: &'static str,
    pub privacy: &'static str,
    pub network: &'static str,
    pub peripherals: &'static str,
    pub brightness: &'static str,
    pub media: &'static str,
    pub main_display: &'static str,
    pub set_main: &'static str,
    pub fit_icon: &'static str,
    pub fit_short: &'static str,
    pub fit_pill: &'static str,
    pub fit_vertical: &'static str,
    pub fit_horizontal: &'static str,
    pub fit_full: &'static str,
    pub bar_only: &'static str,
    pub on_bar: &'static str,
    pub size: &'static str,
    pub tap_to_adjust: &'static str,
}

const PT: Texts = Texts {
    always: "Sempre",
    when_active: "Ativo",
    never: "Nunca",
    privacy: "Câmera e microfone",
    network: "Rede",
    peripherals: "Periféricos",
    brightness: "Brilho",
    media: "Mídia",
    main_display: "Monitor principal",
    set_main: "Tornar principal",
    fit_icon: "Ícone",
    fit_short: "Pílula curta",
    fit_pill: "Pílula",
    fit_vertical: "Vertical",
    fit_horizontal: "Horizontal",
    fit_full: "Inteiro",
    bar_only: "Só na barra",
    on_bar: "Na barra",
    size: "Tamanho",
    tap_to_adjust: "Toque num item para ajustar, arraste para mudar de lugar",
};

const EN: Texts = Texts {
    always: "Always",
    when_active: "On",
    never: "Never",
    privacy: "Camera, microphone",
    network: "Network",
    peripherals: "Peripheral batteries",
    brightness: "Brightness",
    media: "Media",
    main_display: "Main monitor",
    set_main: "Make main",
    fit_icon: "Icon",
    fit_short: "Short pill",
    fit_pill: "Pill",
    fit_vertical: "Vertical",
    fit_horizontal: "Horizontal",
    fit_full: "Full",
    bar_only: "Panel only",
    on_bar: "On the panel",
    size: "Size",
    tap_to_adjust: "Tap an item to adjust it, drag it to move it",
};

pub fn texts() -> &'static Texts {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|l| !l.is_empty()))
        .unwrap_or_default();
    if lang.starts_with("pt") { &PT } else { &EN }
}
