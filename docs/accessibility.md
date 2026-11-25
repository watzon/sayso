# Accessibility

Status: all phases are on the branch `accessibility` since 2026-10-17, not committed. The plan was written on 2026-10-17 and changed the same day after a review by Codex (gpt-6-astra), see "Review record". See "Result" for what was checked and what is still open.

This file says what GPUI and gpui-kit give us for accessibility, what Sayso does with it today, and how we change the app so that screen readers, voice control, and computer-use agents can read and operate every window.

## What we found

### The stack supports accessibility

- `gpui-pre` 0.3.7 (our GPUI) includes the AccessKit integration from Zed PR [#56065](https://github.com/zed-industries/zed/pull/56065), merged 2026-05-27. It sends one AccessKit tree per window to the platform adapter: NSAccessibility on macOS, UI Automation on Windows, AT-SPI on Linux.
- A GPUI element gets a node in the tree only when it has an `.id(...)` **and** a `.role(...)`. Without both, it does not exist for assistive technology.
- `div()` has builders for `role`, `aria_label`, `aria_description`, `aria_value`, `aria_toggled`, `aria_selected`, `aria_expanded`, numeric values with min, max, and step, `aria_level`, position and size in a set, row and column indexes, `aria_keyshortcuts`, and `aria_active_descendant`.
- `.on_click(...)` also registers the AccessKit `Click` action. So when an element has a node, VoiceOver, Voice Control, and Cua can press it. `on_a11y_action` adds other actions (Increment, Decrement, SetValue, and so on).
- `a11y_synthetic_children` lets an element edit its own node or add child nodes. This is how we can set properties that have no builder, for example `Live::Polite`.
- `Text` from the `text!` macro gets `Role::Label`. A plain `&str` or `SharedString` child gets **no node**.
- `gpui-base` (in gpui-kit) has unstyled controls with roles, labels, keyboard activation (Enter and Space), focus, and state: `Button`, `Switch`, `Checkbox`, `Radio`/`RadioGroup`, `Toggle`/`ToggleGroup`, `Tabs`, `Select`, `Slider`, `Progress`, `Dialog`, `AlertDialog`, `FocusTrap`, `Tooltip`. It is made for apps that own their visual style, as Sayso does.
- gpui-component `Input` and `Textarea` (which Sayso already uses) expose `TextInput` and `MultilineTextInput` with their value.
- `Window::debug_a11y_tree_json()` returns the last tree as JSON. The gpui-base tests use `write_a11y_info` with `TestAppContext` to assert role, label, and state. We can test the same way.

### Gaps in the stack

- No `aria-disabled` builder. gpui-base notes this in `switch.rs`. A disabled control looks enabled to a screen reader.
- No live-region builder and no announcement API. AccessKit on macOS posts `NSAccessibilityAnnouncementRequested` when the **value** of a live node changes (`accesskit_macos` `event.rs`). A changed label or a role (`Alert`, `Status`) alone gives no announcement. So a live node needs `aria_value` and `Live` set through `a11y_synthetic_children`.
- `a11y_synthetic_children` gets the node of its element as `&mut accesskit::Node`, so it can also set the disabled state. An element has one such closure; a second call replaces the first.
- `FocusTrapContainer` in gpui-base does not forward the role of its child, so a `Dialog` inside it loses its node. The dialog role must be on an element outside the trap, or on a plain `div`.
- `Tabs` in gpui-base has no keyboard navigation, `RadioGroup` gives only the container role, and `ToggleGroup` is a `Toolbar`. We add the keyboard behavior ourselves.
- macOS ignores `aria-current`. It shows a `Tab` as a radio button with a tab subrole, with its value from the selected state.
- Test windows (`TestAppContext`) never activate accessibility, because the test platform has no adapter. `debug_a11y_tree_json()` is `None` in a unit test. A unit test can only call `write_a11y_info` on one element.
- In **debug builds**, some tree errors panic: a duplicate node id, two nodes that set focus in one frame, two active descendants. In release builds a duplicate node is discarded without a message, and the others log a warning. Duplicate ids are the main risk when we add ids to text in lists. `scripts/dev.sh` makes a debug build, so a duplicate id stops a dev run as soon as VoiceOver or Cua connects.
- Platform status upstream: VoiceOver and NVDA work. On Linux, only AT-SPI node visibility was checked; nobody tested Orca.
- Zed itself ships with AccessKit off (`Application::new_inaccessible`, PR [#57954](https://github.com/zed-industries/zed/pull/57954)). Sayso is one of the first apps to rely on it.

### Sayso today

- Sayso sets no role, no label, and no accessibility action anywhere. `grep` found no `.role(`, `aria_`, or `a11y` in the Rust app.
- `sayso-ui` builds `Button`, `Switch`, `Segmented`, `NavItem`, `Chip`, `Stepper`, `Progress`, `radio_dot`, and `option_card` from plain `div`s. It uses gpui-component only for `Input`, `Textarea`, scrolling, and one tooltip.
- There are about 215 `.on_click` handlers in `sayso-app` and `sayso-ui`, and about 145 uses of the `sayso-ui` controls.
- I read the tree of the onboarding window with Cua. It has the window, two traffic-light buttons, and the menu bar item. It has nothing from Sayso, not even "Get started". This is why Cua cannot press anything in the background: there is no element to press.
- The console lines come from GPUI:
  - `INFO gpui::window] Accessibility activated` (an AT client connected; this one is normal).
  - `INFO gpui::window::a11y] a11y: focused element (...) has no accessibility node (it has an id but no role)` and the same with "(it has no element id)". GPUI writes one each time focus moves to an element without a node. The fix is a role on every focusable element, not a log filter.
- We pinned `gpui-kit = "=0.7.0"`. The `accessibility` branch moves to 0.7.3 (2026-10-12, gpui-pre 0.3.9). It built with no code change. It fixes a debug-build abort when a context menu opens with accessibility active (gpui-kit PR #3422) and adds labels to some built-in controls. 0.7.2 is yanked.

## Goals

1. Every control in every Sayso window has a node with a correct role, a name, and its state (on, off, selected, expanded, value).
2. Every control works from the keyboard: Tab reaches it, Enter or Space activates it, Escape closes popups and dialogs, and the focus is visible.
3. Each change of dictation state (recording, processing, inserted, failed) is announced.
4. Cua `get_window_state` lists the Sayso controls, and an element-indexed click works in the background.
5. The `a11y:` lines go away in normal use of the Hub, onboarding, and the popover.
6. Tests catch a control that has no node or no name.

Out of scope for this plan: color contrast, text size, and reduced motion. gpui-base has `reduce_motion`; we can do that work after this plan.

## Plan

### Phase 0: upgrade and spike (small)

- Move to `gpui-kit = "=0.7.3"`. Read the 0.7.1 and 0.7.3 changelogs for API breaks. Run the full build on macOS, Linux, and Windows.
- Spike 1, live announcements. In the overlay, give the status text `Role::Status` and set `Live::Polite` through `a11y_synthetic_children`. Check with VoiceOver that a change is spoken. The overlay is a panel that does not take focus, so this can fail. If it fails, the fallback is a live node in the Hub window, or a direct `NSAccessibilityPostNotificationWithUserInfo` call in `sayso-platform-macos`.
- Spike 2, `aria-disabled`. Find out whether we can set the disabled state through `a11y_synthetic_children` (`parent_node().set_disabled()`). If not, propose a builder upstream.

Exit: the upgrade builds and runs, and we know which path gives announcements.

### Phase 1: accessible `sayso-ui` (main work)

`sayso-ui` is the narrow point. Most of the app gets roles from this phase alone.

- Rebuild the controls on gpui-base and keep their look:

  | Sayso control | gpui-base base | Role and state |
  |---|---|---|
  | `Button` | `base::Button` | Button, label, disabled |
  | `Switch` | `base::Switch` | Switch, toggled, label |
  | `Segmented` | `ToggleGroup` or `RadioGroup` | RadioGroup with Radio items, selected |
  | `NavItem` | `base::Button` | Tab in a TabList, `aria_selected`, position in set; the page is a named group |
  | `Chip` | `base::Button` | by use: Button with toggled (filter), RadioButton (one of a set), Button with expanded (opens a menu), Button (command) |
  | `Stepper` | `base::Button` | SpinButton with the value as text, Increment and Decrement actions, Up and Down keys; − and + are not tab stops |
  | `Progress` | `base::Progress` | ProgressIndicator, value |
  | `option_card` + `radio_dot` | `Radio` in `RadioGroup` | Radio, selected, position in set |

- Make a label required where the control has no visible text. `NavItem::icon_only` and every icon button must take a catalog string (`t!`). Make this a constructor argument, so the compiler catches a missing label.
- Text helpers in `text.rs`: `display` and `title` give `Role::Heading` with `aria_level`. Other text helpers give a `Label` node. The id comes from the call site and the text, so the same helper in a list needs a row id above it. Text inside a control stays without a node, because the control carries the name. Content that is only text (transcripts, onboarding steps, permission help, empty states, release notes) must show.
- `Segmented` and single-choice option cards: one tab stop on the selected item, and the arrow keys change the selection (the APG radio pattern).
- `Banner` gets `Role::Alert` for errors and `Role::Status` otherwise. Its text is the value of a live node, so a new banner is announced.
- Visible focus ring: one focus style for all controls. Design it in Paper first.

Exit: unit tests for each control assert role, name, and state through `write_a11y_info`.

### Phase 2: windows and pages (medium, wide)

Go window by window. For each one, replace raw `div().on_click` with a `sayso-ui` control, or add `.role()` and a label where a control does not fit.

1. Onboarding (7 steps). It is the first thing a new user sees, and a blind user cannot get past "Get started" today.
2. Popover: it has mixed controls, so it is a named panel (a `Dialog` node), not a menu. Initial focus on the first control, Escape closes it. The status item is native and outside GPUI; check that AXPress opens the popover.
3. Hub shell: sidebar as `Navigation`, page content as a `Group` named for the page, title bar buttons.
4. Hub pages, in order of use: Home, Settings sections, Models, History, Dictionary, Styles.
   - Lists (History rows, models, dictionary words, styles): `List` and `ListItem` with position and size. Each row id comes from the record id, not from the index, so the node stays the same when the list changes.
   - Dialogs and confirm prompts: `Dialog` or `AlertDialog` with `FocusTrap`, focus goes back to the trigger on close.
5. Update and What's New windows, model picker.

Exit for each window: a VoiceOver pass reads every control in order, and Cua lists and presses each control.

### Phase 3: custom drawing and the overlay (small, risky)

- `pen_line` in the model picker: `Role::Slider` with value, min, max, and step, and Increment, Decrement, and SetValue actions. Arrow keys move it.
- Waveforms and audio bars: decoration, so no node. The history audio player gets a play/pause `Button` and a `Slider` for the position.
- Overlay: the status node from spike 1. Name the state in words ("Recording", "Processing", "Inserted", "Failed: <reason>"), never only a color.
- The overlay can never be the key window, so its buttons (Undo, Dismiss, Copy) are not keyboard controls. They get nodes for VoiceOver and Cua, and each one must also exist in a window that takes focus (the Hub or the popover) or as a hotkey.
- Inputs: each field gets a name from a catalog string, not only a placeholder. A key field is a password field.

### Phase 4: checks that stay (small)

- Unit tests for each `sayso-ui` control, with `write_a11y_info`: role, name, state, and the actions it supports. Blocked for now: the `test-support` feature of gpui-kit 0.7.3 adds `gpui-fast-macos`, which pins `cocoa = 0.26.0`, and gpui-pre 0.3.9 has `cocoa 0.26.1` in the lock. Without that feature there is no `TestAppContext`. Each control builds its `Pressable` in `pressable(cx)`, so the tests can read the node when the feature resolves. The pure parts (`spoken_keys`, `call_site_id`, the audit check) have unit tests.
- A dev audit: with `SAYSO_A11Y_AUDIT=1`, Sayso checks the tree of each window when an AT client is connected, and logs each node with a role that needs a name but has none, and each focus on an element without a node. Unit tests cannot do this, because test windows never activate accessibility.
- A Cua pass on each window: `get_window_state` lists the controls, and a background element click works.
- A short manual checklist in `docs/accessibility.md` for each release: VoiceOver (macOS), NVDA (Windows), Orca (Linux, GNOME VM).
- Optional: `gpui-testing-library` 0.1.0 has role and label queries. It is new and has one author, so we try it in phase 4 and do not depend on it before then.

## Conventions for UI code

These rules apply to every window. The building blocks are the `Key`, `Role`, and label calls of the kit in `internal/ui/kit.go`.

- **A control is a `Pressable` or a `sayso-ui` control.** `Pressable::new(id)` is a `div` with an id, a role (default `Button`), a focus handle (so Tab reaches it and Enter or Space presses it), the focus ring, and `.disabled(..)`. It adds no layout. Use its own `.on_click(..)`, which a disabled control drops. Use `.shadows(..)` for its shadows, so the focus ring adds to them. Never put `.on_click` on a plain `div`: it has no node and no keyboard access.
- **Every control has a name.** `Button`, `Chip`, and `NavItem` use their visible text. Give `.label(..)` (or `.a11y_label(..)`) to a `Switch`, `Segmented`, `Stepper`, `Progress`, an icon-only control, an `Input` (`.aria_label(..)`), and a control that has a vague text such as "Change" or "Remove". Use the row title as the name, and add the object when there are many of them: "Delete Parakeet". Names come from the catalogs (`t!`), never from English literals.
- **Pick the role by what the control does.** One choice of a set: `.a11y_radio(name, checked, index, count)`, in a parent with `Role::RadioGroup` and a name. On and off: `Role::Switch` or `Role::CheckBox` with `aria_toggled`. Opens a menu or popup: `aria_expanded(open)`. A sidebar or page list: `NavItem` (a `Tab`) in a `Role::TabList`. A chip: `ChipRole`.
- **Text goes through `sayso_ui::text`.** Each helper gives a node. `display`, `letterpress`, and `title` are headings, `caps_heading` is a section heading. A plain string child (`.child("...")`, `.child(t!(..))`) has no node: use it only for text inside a control, or for decoration. For text that is not in a helper, use `A11yText::new(text)`.
- **Ids must be unique among siblings.** A text helper takes its id from the call site and the text. A list row must have an `.id(..)` from its data (the model id, the record id), not from its index when the list can change. Two nodes with one id stop a debug build when VoiceOver or Cua is connected.
- **A message that appears is live; a state that changes is a `LiveStatus`.** AccessKit announces a live node when it appears, and also when its value changes. So `.a11y_live(text, assertive)` is for a message that shows after an event: an error, a question, "Copied". `Banner` and `kit::notice` are live already. A state that is on the page from the start (the state of a model, the engine, a permission, a count) is a `LiveStatus::new(id, text)`: it is read when the value changes, not when the page opens. Both need a stable id. macOS reads the value, so the text must be the value. Text inside a live node is not live itself (`A11yText` sets `Live::Off`), else a clock inside it is read every second.
- **A value is not a heading.** Use `text::figure` or `text::display_figure` for "93.7%" or "1.9 GB" in the title face.
- **Focus goes to an element with a node.** A dialog takes the focus on its own `Role::Dialog` element, not on its backdrop. The key recorder takes the focus on `kit::key_listener`, a named empty element. A view that must move the focus to a control (back to a trigger, to the chosen radio button) gives the control its own handle with `Pressable::track_focus_handle`.
- **Decoration has no node.** Icons, dots, waveforms, and seals stay plain. Do not give a role to a `div` that only groups layout.
- **Windows outside the kit root need Tab.** `gpui_kit::open_window` binds Tab and Shift+Tab. A window from plain `cx.open_window` must bind them itself.
- **Keyboard inside a composite.** In a radio group or a tab list, the arrow keys move the selection or the focus. `Segmented` does this already.

## Result

What exists on the branch:

- `sayso-ui`: `a11y.rs` (`Pressable`, `A11yExt`, `A11yText`, `LiveStatus`, `call_site_id`, `spoken_keys`, `focus_ring`), and every control in `controls.rs` and `basics.rs` on it. The text helpers give nodes.
- `sayso-app`: every window (onboarding, Hub pages and settings, popover, overlay, What's New, update, model picker) has roles, names, and states. `a11y.rs` redraws a window once when an assistive client connects to it (GPUI sends no tree in the first frame after activation, see below) and has the audit (`SAYSO_A11Y_AUDIT=1`) and a tree dump (`SAYSO_A11Y_DUMP=<folder>`).
- `sayso-platform-macos`: the menu bar icon has a name and a press action, so VoiceOver and Cua can open the popover.
- A Cua press on a control works also when the control is scrolled out of view: `Pressable` answers the Click action itself. For a plain `.on_click`, GPUI clicks the center of the node, which misses there.
- The popover stays open when an assistive client opens it without making Sayso active.

Checked on macOS on 2026-10-17, in a debug build with Cua and AX scripts:

- Each Hub page, each settings page, each onboarding step, What's New, the popover, and the overlay show their controls with names and states. The audit found no problem on them. Background element clicks work: tabs, radio buttons, switches, steppers, dialogs, the popover, and the overlay.
- The overlay announces "Recording", "Transcribing", and "No speech was heard." (AXAnnouncementRequested, read with an AX observer). A page that opens is quiet.

Not checked, or open:

- VoiceOver itself, the keyboard path (Tab, arrow keys, Escape), and the look of the focus ring. A keyboard test needs the window in front, so it is a manual test. The focus ring has no Paper design yet: two rings, paper then ink, so it differs from the selection ring.
- Windows (NVDA) and Linux (Orca). The code builds and passes clippy on Linux; nothing ran there.
- Unit tests of the controls: blocked by the `cocoa` conflict of the gpui-kit `test-support` feature (phase 4).
- A window behind another Sayso window does not draw, so its tree can be late until it comes to the front.
- The practice field (gpui-component `Textarea`) takes the focus on an inner element without a node (gpui-kit issue #3183).
- Undo after a cancel exists only in the overlay, which never takes the keyboard focus. VoiceOver and Cua can press it; the keyboard cannot.
- GPUI has no scroll-on-focus, so Tab can move the focus to a control out of view. The arrow keys in History scroll the row into view.
- Menus in the Hub do not take the focus when they open.

Test instances: a second Sayso with a copy of the real config registers the same global hotkeys and records when the user dictates. A test instance needs empty hotkeys and a fake microphone.

## Risks

- **Duplicate ids panic in debug builds.** The worst case is text in lists. Rule: list rows take their id from the data, and text inside a row gets no own id unless it needs one.
- **gpui-kit API changes.** We pin exact versions because snapshots break. Each upgrade is its own commit with a full build on three systems.
- **Overlay announcements may not work** from a panel that never takes focus. Spike 1 answers this before we build on it.
- **Linux is the least tested.** Upstream never tested Orca. We test on the GNOME VM and report what fails upstream.
- **Labels add catalog strings.** Each new `t!` key needs the 4 translations. Accessible names are short ("Collapse sidebar", "Delete word"), so the cost is small.

## Review record

Codex (gpt-6-astra, reasoning high) reviewed the plan on 2026-10-17. All 12 findings were checked against the sources and accepted:

1. `FocusTrapContainer` loses the dialog role. Dialogs put the role outside the trap.
2. Test windows do not activate accessibility. Phase 4 uses unit tests per control, a dev audit, and Cua instead of a tree test.
3. A tree cannot show what is missing from it. The audit also logs focus on elements without a node.
4. macOS announces the value of a live node, not a label. Live nodes use `aria_value`.
5. Body text must not be hidden by default. Text helpers give nodes.
6. gpui-base `Tabs`, `RadioGroup`, and `ToggleGroup` have no keyboard model. We add arrow keys.
7. NavItem is a Tab with an explicit selected state.
8. Chip semantics depend on the use.
9. The popover is a panel, not a menu.
10. Overlay actions need a keyboard equivalent elsewhere.
11. Inputs need names; scroll containers need checks.
12. Duplicate ids are discarded without a message in release. The disabled state works through `a11y_synthetic_children`, so no spike.

## Open questions

None. Chris asked for one review round and all phases on 2026-10-17. The upgrade is the first commit of the branch, body text gets nodes (finding 5), and onboarding goes first.
