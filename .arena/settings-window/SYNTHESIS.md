# Synthesized design: settings window

## Synthesis decision

Base: Candidate 1 (`SettingsCommand` + one `apply`, modeless dialog).
Cross-judge: [Arena cross-judge settings](3287d620-390c-44d6-a8ec-fd214836927e) scored 28/30 vs 16/30. Parent pick agrees.

Why base: one absolute-command write path; deletes widget `WM_COMMAND` / `IDM_*`; modeless custom class avoids disabling `Shell_TrayWnd`; small public surface; no domain-type relocation.

### Grafts from Candidate 2

1. Pure `Visibility` + `effects_for(change, EffectFacts) -> Effects` so guards and effect selection unit-test without an `AppState` fixture. `mutate` only writes fields.
2. `PollInterval::from_stored` / `AlertThreshold::from_stored` inside `normalize_settings` so memory never holds a non-preset value.
3. Single `display_key(taskbar)` helper shared by read and write paths.

### Rejected

- Modal `PropertySheetW` owned by the embedded widget (disables Explorer taskbar).
- Draft / `SettingsPatch` / Apply-Cancel second owner of settings.
- `ToggleX` alongside `SetX`, and fake menu ids for routing.
- Moving `LanguageId` / `Appearance` out of `window` in this change.
- Fanning new strings to all 11 locales in MVP (English + existing en/zh fallback).

### Defaults applied (operator can override later)

- Dialog owner = widget HWND, as grounding. Spike Alt-Tab / topmost before the dialog commit only.
- New labels English-first.
- First verifiable unit: menu → `apply` with no dialog UI yet.

## Shape (grafted)

See `.arena/settings-window/candidate-1/DESIGN.md` for full type sketch and call sites.

Additions:

```rust
struct EffectFacts { has_poll_result: bool, claude_available: bool }

fn effects_for(change: &Change, before: &AppStateSnapshot, facts: EffectFacts) -> Effects {
    // pure; unit-tested
}

fn display_key(taskbar: &TaskbarWindow) -> String {
    // taskbar_monitor_device or format!("{}", index+1)
}
```

`normalize_settings` snaps `poll_interval_ms` and `alert_threshold_percent` through `from_stored`.

## Next implementation step

Create `src/window/settings.rs` with `SettingsCommand`, pure `Visibility` / `effects_for`, `prepare` / `mutate` / `run_effects`, and `apply`. Port every settings menu arm onto `MenuBuilder` + `TPM_RETURNCMD`. Delete `IDM_*` settings constants and the widget settings `WM_COMMAND` arms. No dialog yet. `cargo test` + manual tray menu toggles green before the dialog unit.
