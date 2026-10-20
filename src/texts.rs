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
    pub main_display: &'static str,
    pub set_main: &'static str,
    pub divider: &'static str,
    pub bubble: &'static str,
    pub save: &'static str,
    pub cancel: &'static str,
    pub add_to: &'static str,
    pub limit: &'static str,
    pub bar_hint: &'static str,
    pub restore: &'static str,
    pub structure: &'static str,
    pub bubble_word: &'static str,
    pub add_here: &'static str,
    pub nothing_playing: &'static str,
    pub fold: &'static str,
    pub unfold: &'static str,
    pub bar_only: &'static str,
    pub on_bar: &'static str,
}

const PT: Texts = Texts {
    always: "Sempre",
    when_active: "Ativo",
    never: "Nunca",
    privacy: "Câmera e microfone",
    network: "Rede",
    peripherals: "Periféricos",
    brightness: "Brilho",
    main_display: "Monitor principal",
    set_main: "Tornar principal",
    divider: "Divisória",
    bubble: "Nova bolha",
    save: "Salvar",
    cancel: "Cancelar",
    add_to: "Adicionar na bolha",
    limit: "Até 3 bolhas, com até 2 divisórias cada",
    bar_hint: "Clique em um ícone para editar se e quando ele aparece na barra",
    restore: "Restaurar",
    structure: "Bolhas e divisórias",
    bubble_word: "Bolha",
    add_here: "Adicionar nesta bolha",
    nothing_playing: "Nada tocando",
    fold: "Recolher",
    unfold: "Expandir",
    bar_only: "Só na barra",
    on_bar: "Na barra",
};

const EN: Texts = Texts {
    always: "Always",
    when_active: "On",
    never: "Never",
    privacy: "Camera, microphone",
    network: "Network",
    peripherals: "Peripheral batteries",
    brightness: "Brightness",
    main_display: "Main monitor",
    set_main: "Make main",
    divider: "Divider",
    bubble: "New bubble",
    save: "Save",
    cancel: "Cancel",
    add_to: "Add to bubble",
    limit: "Up to 3 bubbles, with up to 2 dividers each",
    bar_hint: "Click an icon to set whether and when it shows on the panel",
    restore: "Restore",
    structure: "Bubbles and dividers",
    bubble_word: "Bubble",
    add_here: "Add to this bubble",
    nothing_playing: "Nothing playing",
    fold: "Fold",
    unfold: "Unfold",
    bar_only: "Panel only",
    on_bar: "On the panel",
};

pub fn texts() -> &'static Texts {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|l| !l.is_empty()))
        .unwrap_or_default();
    if lang.starts_with("pt") { &PT } else { &EN }
}
