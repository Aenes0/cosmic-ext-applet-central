// SPDX-License-Identifier: GPL-3.0-only

/// Um item presente, em ordem de registro: (menu_id, chave se já conhecida).
pub type Present<'a> = (usize, Option<&'a str>);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Split {
    /// menu_ids na ordem da barra.
    pub bar: Vec<usize>,
    /// menu_ids na caixa.
    pub boxed: Vec<usize>,
    pub show_arrow: bool,
}

/// Quantos botões cabem no eixo maior; `None` = sem limite (painel não mandou bounds).
pub fn capacity(major_axis_len: Option<f32>, button_len: f32) -> Option<usize> {
    let len = major_axis_len?;
    if button_len <= 0.0 {
        return None;
    }
    Some((len / button_len).floor().max(0.0) as usize)
}

/// Decide quem fica na barra e quem vai para a caixa.
/// `force_arrow`: a ^ aparece mesmo sem escondidos (arrasto começado na barra).
pub fn split(
    items: &[Present<'_>],
    visible: &[String],
    capacity: Option<usize>,
    force_arrow: bool,
) -> Split {
    let position = |key: &str| visible.iter().position(|v| v == key);

    let mut candidates: Vec<(usize, usize)> = Vec::new(); // (posição em visible, menu_id)
    let mut hidden: Vec<usize> = Vec::new();
    for &(id, key) in items {
        match key.and_then(position) {
            Some(pos) => candidates.push((pos, id)),
            None => hidden.push(id),
        }
    }
    // Ordenação estável: empate (mesma chave) mantém a ordem de registro.
    candidates.sort_by_key(|&(pos, _)| pos);
    let mut bar: Vec<usize> = candidates.into_iter().map(|(_, id)| id).collect();

    let mut show_arrow = !hidden.is_empty() || force_arrow;
    let mut boxed = Vec::new();
    if let Some(c) = capacity
        && bar.len() + show_arrow as usize > c
    {
        show_arrow = true;
        boxed = bar.split_off(c.saturating_sub(1).min(bar.len()));
    }
    boxed.extend(hidden);
    Split { bar, boxed, show_arrow }
}

/// Índice do vão (0..=len) sob a coordenada `pos` do eixo maior, relativa ao início da fila.
pub fn drop_slot(len: usize, button_len: f32, pos: f32) -> usize {
    if button_len.is_nan() || button_len <= 0.0 {
        return len;
    }
    if pos < 0.0 {
        return 0;
    }
    (((pos + button_len / 2.0) / button_len).floor() as usize).min(len)
}

/// Nova `visible` com `key` posta antes do item mostrado no vão `slot`.
/// `shown`: chaves da barra como desenhada (com repetidas), SEM as ocorrências de `key`.
pub fn show_at(visible: &[String], shown: &[&str], key: &str, slot: usize) -> Vec<String> {
    let mut out: Vec<String> = visible.iter().filter(|v| v.as_str() != key).cloned().collect();
    let at = shown
        .get(slot)
        .and_then(|target| out.iter().position(|v| v == target))
        .unwrap_or(out.len());
    out.insert(at, key.to_string());
    out
}

/// Converte um vão contado na barra inteira (`keys`, com `key`) para o vão na barra sem as
/// ocorrências de `key` (o `shown` de `show_at`). Para um solto cujo vão foi medido com o item
/// ainda na fila (arrasto vindo de outra instância).
pub fn slot_without(keys: &[&str], key: &str, slot: usize) -> usize {
    let slot = slot.min(keys.len());
    slot - keys[..slot].iter().filter(|k| **k == key).count()
}

/// Nova `visible` sem nenhuma ocorrência de `key`.
pub fn hide(visible: &[String], key: &str) -> Vec<String> {
    visible.iter().filter(|v| v.as_str() != key).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test] fn empty_tray_shows_nothing() {
        assert_eq!(split(&[], &[], Some(10), false), Split::default());
    }
    #[test] fn new_items_go_to_box() {
        assert_eq!(split(&[(1, Some("a"))], &[], None, false),
            Split { bar: vec![], boxed: vec![1], show_arrow: true });
    }
    #[test] fn unknown_identity_goes_to_box() {
        assert_eq!(split(&[(1, None)], &s(&["a"]), None, false).boxed, vec![1]);
    }
    #[test] fn bar_follows_config_order_and_arrow_hides_when_nothing_hidden() {
        assert_eq!(split(&[(1, Some("a")), (2, Some("b"))], &s(&["b", "a"]), None, false),
            Split { bar: vec![2, 1], boxed: vec![], show_arrow: false });
    }
    #[test] fn same_key_items_follow_together() {
        let r = split(&[(1, Some("a")), (2, Some("b")), (3, Some("a"))], &s(&["a"]), None, false);
        assert_eq!((r.bar, r.boxed), (vec![1, 3], vec![2]));
    }
    #[test] fn duplicate_keys_in_config_render_once() {
        assert_eq!(split(&[(1, Some("a"))], &s(&["a", "a"]), None, false).bar, vec![1]);
    }
    #[test] fn absent_keys_take_no_slot() {
        let r = split(&[(1, Some("a")), (2, Some("b"))], &s(&["gone", "a", "b"]), Some(2), false);
        assert_eq!((r.bar, r.show_arrow), (vec![1, 2], false));
    }
    #[test] fn spill_from_the_end_goes_first_in_box() {
        let items = [(1, Some("a")), (2, Some("b")), (3, Some("c")), (4, Some("d"))];
        assert_eq!(split(&items, &s(&["a", "b", "c"]), Some(3), false),
            Split { bar: vec![1, 2], boxed: vec![3, 4], show_arrow: true });
    }
    #[test] fn spill_brings_the_arrow() {
        let items = [(1, Some("a")), (2, Some("b")), (3, Some("c"))];
        assert_eq!(split(&items, &s(&["a", "b", "c"]), Some(2), false),
            Split { bar: vec![1], boxed: vec![2, 3], show_arrow: true });
    }
    #[test] fn exact_fit_has_no_arrow() {
        let items = [(1, Some("a")), (2, Some("b")), (3, Some("c"))];
        assert_eq!(split(&items, &s(&["a", "b", "c"]), Some(3), false).show_arrow, false);
    }
    #[test] fn force_arrow_reserves_its_slot() {
        let items = [(1, Some("a")), (2, Some("b"))];
        assert_eq!(split(&items, &s(&["a", "b"]), Some(2), true),
            Split { bar: vec![1], boxed: vec![2], show_arrow: true });
    }
    #[test] fn every_item_lands_exactly_once() {
        let items = [(1, Some("a")), (2, None), (3, Some("c")), (4, Some("a")), (5, Some("e"))];
        let visible = s(&["e", "a", "x"]);
        for c in (0..=6).map(Some).chain([None]) {
            for force in [false, true] {
                let r = split(&items, &visible, c, force);
                let mut all: Vec<usize> = r.bar.iter().chain(&r.boxed).copied().collect();
                all.sort();
                assert_eq!(all, vec![1, 2, 3, 4, 5], "capacity {c:?} force {force}");
                if !r.boxed.is_empty() { assert!(r.show_arrow); }
                if let Some(c) = c { assert!(r.bar.len() + r.show_arrow as usize <= c.max(1)); }
            }
        }
    }
    #[test] fn capacity_floors_and_guards() {
        assert_eq!(capacity(Some(130.0), 40.0), Some(3));
        assert_eq!(capacity(None, 40.0), None);
        assert_eq!(capacity(Some(39.0), 40.0), Some(0));
        assert_eq!(capacity(Some(130.0), 0.0), None);
    }
    #[test] fn drop_slot_by_half_button() {
        assert_eq!(drop_slot(3, 40.0, 0.0), 0);
        assert_eq!(drop_slot(3, 40.0, 19.0), 0);
        assert_eq!(drop_slot(3, 40.0, 21.0), 1);
        assert_eq!(drop_slot(3, 40.0, 1000.0), 3);
        assert_eq!(drop_slot(0, 40.0, 50.0), 0);
        assert_eq!(drop_slot(3, 40.0, -5.0), 0);
        assert_eq!(drop_slot(3, 0.0, 10.0), 3);
    }
    #[test] fn show_at_inserts_before_slot() {
        assert_eq!(show_at(&s(&["a", "b"]), &["a", "b"], "c", 1), s(&["a", "c", "b"]));
    }
    #[test] fn show_at_end_appends() {
        assert_eq!(show_at(&s(&["a", "b"]), &["a", "b"], "c", 2), s(&["a", "b", "c"]));
    }
    #[test] fn show_at_reorders_without_duplicating() {
        assert_eq!(show_at(&s(&["a", "b", "c"]), &["b", "c"], "a", 2), s(&["b", "c", "a"]));
        assert_eq!(show_at(&s(&["a", "b", "c"]), &["a", "b"], "c", 0), s(&["c", "a", "b"]));
    }
    #[test] fn show_at_keeps_absent_keys() {
        assert_eq!(show_at(&s(&["x", "a", "b"]), &["a", "b"], "c", 0), s(&["x", "c", "a", "b"]));
    }
    #[test] fn show_at_into_empty_bar() {
        assert_eq!(show_at(&[], &[], "a", 0), s(&["a"]));
    }
    #[test] fn hide_removes_every_occurrence() {
        assert_eq!(hide(&s(&["a", "b", "a"]), "a"), s(&["b"]));
        assert_eq!(hide(&s(&["b"]), "a"), s(&["b"]));
    }
    #[test] fn slot_without_skips_occurrences_of_key_before_the_slot() {
        // barra a,b,c; solto "entre b e c" (vão 2 contado com a) arrastando a de outro monitor
        assert_eq!(slot_without(&["a", "b", "c"], "a", 2), 1);
        assert_eq!(show_at(&s(&["a", "b", "c"]), &["b", "c"], "a", slot_without(&["a", "b", "c"], "a", 2)),
            s(&["b", "a", "c"]));
    }
    #[test] fn slot_without_ignores_occurrences_at_or_after_the_slot() {
        assert_eq!(slot_without(&["a", "b", "c"], "c", 1), 1);
        assert_eq!(slot_without(&["a", "b", "c"], "x", 3), 3);
        assert_eq!(slot_without(&["a", "b", "a"], "a", 3), 1);
        assert_eq!(slot_without(&["a"], "a", 9), 0);
    }
    #[test] fn showing_a_hidden_item_into_a_full_bar_spills_the_last() {
        // integra com split: c entra no vão 0; d continua escondido; cabem 2 + ^
        let v = show_at(&s(&["a", "b"]), &["a", "b"], "c", 0);
        let items = [(1, Some("a")), (2, Some("b")), (3, Some("c")), (4, Some("d"))];
        assert_eq!(split(&items, &v, Some(3), false),
            Split { bar: vec![3, 1], boxed: vec![2, 4], show_arrow: true });
    }
}
