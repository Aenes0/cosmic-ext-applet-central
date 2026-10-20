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
    pub tiles: &'static str,
    pub media: &'static str,
    pub bar_icons: &'static str,
    pub bar_icons_hint: &'static str,
    pub sections: &'static str,
    pub sections_hint: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub main_display: &'static str,
    pub set_main: &'static str,
}

const PT: Texts = Texts {
    always: "Sempre",
    when_active: "Ativo",
    never: "Nunca",
    privacy: "Câmera e microfone",
    network: "Rede",
    peripherals: "Periféricos",
    brightness: "Brilho",
    tiles: "Blocos",
    media: "Mídia",
    bar_icons: "Ícones na barra",
    bar_icons_hint: "Quando cada ícone aparece na barra. O bloco continua no painel.",
    sections: "Ordem do painel",
    sections_hint: "Suba ou desça cada parte do painel.",
    up: "Subir",
    down: "Descer",
    main_display: "Monitor principal",
    set_main: "Tornar principal",
};

const EN: Texts = Texts {
    always: "Always",
    when_active: "On",
    never: "Never",
    privacy: "Camera, microphone",
    network: "Network",
    peripherals: "Peripheral batteries",
    brightness: "Brightness",
    tiles: "Tiles",
    media: "Media",
    bar_icons: "Panel icons",
    bar_icons_hint: "When each icon shows on the panel. The tile stays in the popup.",
    sections: "Popup order",
    sections_hint: "Move each part of the popup up or down.",
    up: "Up",
    down: "Down",
    main_display: "Main monitor",
    set_main: "Make main",
};

pub fn texts() -> &'static Texts {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|l| !l.is_empty()))
        .unwrap_or_default();
    if lang.starts_with("pt") { &PT } else { &EN }
}
