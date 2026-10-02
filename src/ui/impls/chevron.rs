//! The folding chevron, shared by every list that folds a group away.
//!
//! One icon, rotated — not two icons swapped. The swap answered "which way is it
//! pointing" but skipped how it got there, and a heading whose arrow changes
//! species in one frame reads as a flicker rather than a fold. The rotation is
//! driven per toggle: a counter is bumped each time a group turns, and the
//! counter both salts the animation's element id (so the next toggle starts a
//! new swing instead of continuing the old clock) and tells the icon to animate
//! at all. A group never toggled draws its angle statically, so first paint has
//! no animation and a re-render for unrelated reasons — a search keystroke, a
//! selection — does not re-sweep arrows that did not move.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    prelude::*, Animation, AnimationExt as _, AnyElement, Hsla, SharedString, Transformation,
    percentage,
};

/// How long one toggle's swing takes. Fast enough to stay with the click,
/// slow enough that the eye sees a turn rather than a snap.
const SWING: Duration = Duration::from_millis(160);

/// Per-group toggle counters, shared between the list state and its click
/// handlers. Ref-cell'd because a click handler receives an `App`, not the
/// view that owns the map.
pub(crate) type TurnCounter = Rc<std::cell::RefCell<HashMap<String, u64>>>;

pub(crate) fn new_turn_counter() -> TurnCounter {
    Rc::new(std::cell::RefCell::new(HashMap::new()))
}

/// Record that `group` just turned. Call when a fold toggles, before the
/// repaint that should animate it.
pub(crate) fn bump_turn(counter: &TurnCounter, group: &str) {
    if let Ok(mut map) = counter.try_borrow_mut() {
        *map.entry(group.to_string()).or_insert(0) += 1;
    }
}

/// The chevron for a group header: pointing down while open, right while
/// folded, and swinging between the two when `turn` says this is the toggle
/// that just happened. `group` names the header's element — two groups share
/// one counter space, so the animation id carries it to stay one of a kind
/// among siblings.
pub(crate) fn folding_chevron(
    group: &str,
    folded: bool,
    turn: Option<u64>,
    color: Hsla,
) -> AnyElement {
    // Angles in degrees; a folded chevron is the open one turned a quarter
    // turn. Down-to-right is counterclockwise, hence the negative angle.
    let target = if folded { -90.0 } else { 0.0 };
    let icon = Icon::new(IconName::ChevronDown).size_3().text_color(color);
    match turn {
        Some(epoch) => {
            // The swing starts from the angle the icon had before this
            // toggle — the other state's angle — so the turn covers exactly
            // the ground between the two resting poses, in one direction,
            // whichever way this toggle went.
            let start = if folded { 0.0 } else { -90.0 };
            icon.with_animation(
                SharedString::from(format!("folding-chevron-{group}-{epoch}")),
                Animation::new(SWING),
                move |icon, delta| {
                    let angle = start + (target - start) * delta;
                    icon.transform(Transformation::rotate(percentage(angle / 360.0)))
                },
            )
            .into_any_element()
        }
        None => icon
            .transform(Transformation::rotate(percentage(target / 360.0)))
            .into_any_element(),
    }
}
