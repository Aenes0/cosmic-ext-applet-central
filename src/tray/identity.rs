// Copyright 2026 Aenes0
// SPDX-License-Identifier: GPL-3.0-only

/// Identity of a background app's tray item: the key is fixed at the first
/// read that yields one, while the label follows the latest read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Identity {
    key: Option<String>,
    label: Option<String>,
}

impl Identity {
    /// Primeira leitura que dá chave a fixa; leituras seguintes só atualizam o rótulo.
    pub fn learn(&mut self, id: &str, title: &str) {
        let id = non_empty(id);
        let title = non_empty(title);

        if self.key.is_none() {
            self.key = id.clone().or_else(|| title.clone());
        }
        self.label = title.or(id);
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Dica da caixa: Title; na falta, o Id.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_prefers_id() {
        let mut i = Identity::default();
        i.learn("org.kde.foo", "Foo");
        assert_eq!((i.key(), i.label()), (Some("org.kde.foo"), Some("Foo")));
    }

    #[test]
    fn key_falls_back_to_title() {
        let mut i = Identity::default();
        i.learn("  ", "Foo");
        assert_eq!((i.key(), i.label()), (Some("Foo"), Some("Foo")));
    }

    #[test]
    fn no_id_no_title_no_key() {
        let mut i = Identity::default();
        i.learn("", "");
        assert_eq!((i.key(), i.label()), (None, None));
    }

    #[test]
    fn key_frozen_at_first_read() {
        let mut i = Identity::default();
        i.learn("a", "A");
        i.learn("b", "B");
        assert_eq!((i.key(), i.label()), (Some("a"), Some("B")));
    }
}
