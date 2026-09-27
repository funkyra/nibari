use super::*;
use crate::{
    bluetooth::{Adapter, Device, Prompt, PromptKind},
    config::Config,
};

fn fixture() -> Snapshot {
    Snapshot {
        available: true,
        adapters: vec![Adapter {
            path: "/org/bluez/hci0".into(),
            powered: true,
            rate: Some((2048, 98304)),
            ..Default::default()
        }],
        devices: vec![Device {
            path: "/org/bluez/hci0/dev_AA".into(),
            adapter: "/org/bluez/hci0".into(),
            name: "Keyboard".into(),
            icon: "input-keyboard".into(),
            paired: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn bluetooth_popup_uses_its_own_palette_and_accent_for_connected_state() {
    let mut config = Config::default();
    config.network.background = "#102030FF".into();
    config.network.foreground = "#00FF00FF".into();
    config.bluetooth.background = "#442244FF".into();
    config.bluetooth.foreground = "#FFF0E0FF".into();
    config.bluetooth.muted = "#BBAABBFF".into();
    config.bluetooth.accent = "#FFAA77FF".into();
    config.bluetooth.border = "#775566FF".into();
    let palette = Palette::new(&config);

    assert_eq!(palette.background, paint::rgba([0x44, 0x22, 0x44, 0xFF]));
    assert_eq!(palette.foreground, [0xFF, 0xF0, 0xE0, 0xFF]);
    assert_eq!(palette.muted, [0xBB, 0xAA, 0xBB, 0xFF]);
    assert_eq!(palette.border, paint::rgba([0x77, 0x55, 0x66, 0xFF]));
    assert_eq!(palette.connected, [0xFF, 0xAA, 0x77, 0xFF]);
}

#[test]
fn root_offers_connect_without_opening_a_device_page() {
    let mut renderer = Renderer::new(&Config::default());
    let prepared = renderer.prepare_bluetooth(&fixture(), &Page::Root, "", 1, 480);
    assert!(prepared.actions.contains(&UiAction::Act(Action::Connect(
        "/org/bluez/hci0/dev_AA".into()
    ))));
}

#[test]
fn device_details_open_from_icon_instead_of_name() {
    let mut state = fixture();
    state.devices[0].icon = "audio-headphones".into();
    let target = UiAction::Page(Page::Device(state.devices[0].path.clone()));
    let mut renderer = Renderer::new(&Config::default());
    for scale in [1, 2] {
        for width in [260, 480] {
            let popup = renderer.prepare_bluetooth(&state, &Page::Root, "", scale, width);
            let id = popup
                .actions
                .iter()
                .position(|action| action == &target)
                .unwrap() as i32;
            let icon = popup
                .hitboxes
                .iter()
                .find(|hit| hit.selection == MenuSelection::Item(id))
                .unwrap();
            let y = icon.y + icon.height / 2;
            assert_eq!(
                popup.selection_at(40 * scale as i32, y),
                Some(MenuSelection::Item(id))
            );
            assert_eq!(popup.selection_at(90 * scale as i32, y), None);
        }
    }
    let details = renderer.prepare_bluetooth(
        &state,
        &Page::Device(state.devices[0].path.clone()),
        "",
        1,
        480,
    );
    assert!(!details.actions.contains(&target));
}

#[test]
fn power_switch_is_clickable_across_both_halves_in_both_states() {
    let mut renderer = Renderer::new(&Config::default());
    let mut state = fixture();
    for scale in [1, 2] {
        for powered in [false, true] {
            state.adapters[0].powered = powered;
            let popup = renderer.prepare_bluetooth(&state, &Page::Root, "", scale, 480);
            let id = popup
                .actions
                .iter()
                .position(|action| {
                    *action
                        == UiAction::Act(Action::Power(state.adapters[0].path.clone(), !powered))
                })
                .unwrap() as i32;
            for (x, y) in [(383, 25), (418, 38), (452, 51)] {
                assert_eq!(
                    popup.selection_at(x * scale as i32, y * scale as i32),
                    Some(MenuSelection::Item(id))
                );
            }
        }
    }
}

#[test]
fn power_transition_keeps_popup_size_stable() {
    let mut renderer = Renderer::new(&Config::default());
    let mut state = fixture();
    let on = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);

    state.busy = Some(Action::Power(state.adapters[0].path.clone(), false));
    let pending = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    assert_eq!(on.size(), pending.size());
    assert!(pending.selection_at(418, 38).is_none());

    state.busy = None;
    state.adapters[0].powered = false;
    let off = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    assert_eq!(on.size(), off.size());
}

#[test]
fn device_actions_keep_full_paths_and_disconnected_audio_is_not_selectable() {
    let mut renderer = Renderer::new(&Config::default());
    let p = renderer.prepare_bluetooth(
        &fixture(),
        &Page::Device("/org/bluez/hci0/dev_AA".into()),
        "",
        1,
        480,
    );
    assert!(p.actions.contains(&UiAction::Act(Action::Connect(
        "/org/bluez/hci0/dev_AA".into()
    ))));
    assert!(
        !p.actions
            .iter()
            .any(|a| matches!(a, UiAction::Page(Page::Profiles(_) | Page::Codecs(_))))
    );
    let p = renderer.prepare_bluetooth(&fixture(), &Page::Device("/gone".into()), "", 1, 480);
    assert!(!p.actions.iter().any(|a| matches!(a, UiAction::Act(_))));
}

#[test]
fn pairing_requires_valid_input_before_confirmation() {
    let mut state = fixture();
    state.prompt = Some(Prompt {
        id: 12,
        device: state.devices[0].path.clone(),
        text: "Enter PIN".into(),
        kind: PromptKind::Pin,
    });
    let mut renderer = Renderer::new(&Config::default());
    for (input, enabled) in [("", false), ("1234", true)] {
        let p = renderer.prepare_bluetooth(&state, &Page::Root, input, 1, 480);
        let id = p
            .actions
            .iter()
            .position(|a| *a == UiAction::Reply(12, true))
            .unwrap();
        assert_eq!(p.action(id as i32).is_some(), enabled);
    }
}

#[test]
fn scroll_and_keyboard_navigation_reach_every_action_without_overlapping_hits() {
    let mut state = fixture();
    state.devices = (0..20)
        .map(|i| Device {
            path: format!("/org/bluez/hci0/dev_{i}"),
            name: format!("Device {i}"),
            ..state.devices[0].clone()
        })
        .collect();
    let mut renderer = Renderer::new(&Config::default());
    for scale in [1, 2, 3] {
        for width in [260, 480] {
            let mut p = renderer.prepare_bluetooth(&state, &Page::Root, "", scale, width);
            p.constrain(width * scale, 260 * scale);
            let mut selection = None;
            let count = p.controls.iter().filter(|c| c.enabled).count();
            for _ in 0..count {
                selection = p.next_selection(selection, false);
                p.ensure_visible(selection.unwrap());
                let hit = p
                    .hitboxes
                    .iter()
                    .find(|h| Some(h.selection) == selection)
                    .unwrap();
                assert_eq!(
                    p.selection_at(hit.x + hit.width / 2, hit.y + hit.height / 2),
                    selection
                );
                assert!(hit.x >= 0 && hit.x + hit.width <= p.width as i32);
                for other in &p.hitboxes {
                    if other.selection != hit.selection {
                        assert!(
                            intersection(rect_of(hit), rect_of(other)).is_none(),
                            "independent controls overlap"
                        );
                    }
                }
            }
            assert!(p.scroll > 0);
        }
    }
}

fn rect_of(h: &MenuHitbox) -> PixelRect {
    PixelRect {
        x: h.x,
        y: h.y,
        width: h.width,
        height: h.height,
    }
}

#[test]
fn discovery_reordering_preserves_selected_device_identity() {
    let mut state = fixture();
    let mut renderer = Renderer::new(&Config::default());
    let old = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    let wanted = UiAction::Act(Action::Connect(state.devices[0].path.clone()));
    let id = old.actions.iter().position(|a| a == &wanted).unwrap();
    state.devices.insert(
        0,
        Device {
            path: "/org/bluez/hci0/dev_NEW".into(),
            name: "New".into(),
            ..state.devices[0].clone()
        },
    );
    let mut next = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    let selected = next
        .preserve_view(&old, Some(MenuSelection::Item(id as i32)))
        .unwrap();
    let MenuSelection::Item(next_id) = selected else {
        panic!("item selection");
    };
    assert_eq!(next.action(next_id), Some(&wanted));
    state.devices.retain(|d| d.path != "/org/bluez/hci0/dev_AA");
    let mut removed = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    assert!(
        removed
            .preserve_view(&old, Some(MenuSelection::Item(id as i32)))
            .is_none()
    );
}

#[test]
fn traffic_refresh_reuses_unchanged_text_and_evicts_old_rate_labels() {
    let mut renderer = Renderer::new(&Config::default());
    let mut state = fixture();
    let old = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    let sprites = |p: &PreparedBluetooth| {
        p.elements
            .iter()
            .filter_map(|e| match &e.primitive {
                Primitive::Sprite { image, .. } => Some(image.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let before = sprites(&old);
    state.adapters[0].rate = Some((4096, 98304));
    let next = renderer.prepare_bluetooth(&state, &Page::Root, "", 1, 480);
    let after = sprites(&next);
    let shared = after
        .iter()
        .filter(|image| before.iter().any(|old| std::sync::Arc::ptr_eq(old, image)))
        .count();
    assert!(
        shared + 1 >= after.len(),
        "a rate change should only rasterize its changed value"
    );
}
