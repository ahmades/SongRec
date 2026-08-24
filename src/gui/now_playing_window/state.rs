//! Mutable state that is shared by Now Playing event handlers.

use super::background::CachedGradient;
use super::palette::Background;
use super::{BackgroundStyle, NowPlayingSettings, TransitionEffect};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) struct NowPlayingState {
    /// The resolved preference snapshot. Event handlers update this first so
    /// renderers and controls share the same presentation choices.
    pub(super) settings: Rc<Cell<NowPlayingSettings>>,
    /// Prevents GTK notifications emitted by programmatic control updates from
    /// being treated as fresh user preference changes.
    pub(super) applying_settings: Rc<Cell<bool>>,
    pub(super) gradient_surface: Rc<RefCell<Option<CachedGradient>>>,
    pub(super) background_style: Rc<Cell<BackgroundStyle>>,
    pub(super) current_background: Rc<Cell<Background>>,
    pub(super) lights_off: Rc<Cell<bool>>,
    pub(super) showing_listening: Rc<Cell<bool>>,
    pub(super) transition: Rc<Cell<TransitionEffect>>,
    pub(super) transition_duration_ms: Rc<Cell<u64>>,
    /// A user-selected duration waiting for its debounced preference update
    /// to be reflected back by the shared preferences snapshot.
    pub(super) pending_transition_duration_ms: Rc<Cell<Option<u64>>>,
    pub(super) transition_generation: Rc<Cell<u64>>,
    pub(super) transition_duration_update_generation: Rc<Cell<u64>>,
    pub(super) last_track_key: Rc<RefCell<Option<String>>>,
}

impl NowPlayingState {
    pub(super) fn new(settings: NowPlayingSettings) -> Self {
        Self {
            settings: Rc::new(Cell::new(settings)),
            applying_settings: Rc::new(Cell::new(false)),
            gradient_surface: Rc::new(RefCell::new(None)),
            background_style: Rc::new(Cell::new(settings.background_style)),
            current_background: Rc::new(Cell::new(Background::fallback())),
            lights_off: Rc::new(Cell::new(settings.lights_off)),
            showing_listening: Rc::new(Cell::new(true)),
            transition: Rc::new(Cell::new(settings.transition)),
            transition_duration_ms: Rc::new(Cell::new(settings.transition_duration_ms)),
            pending_transition_duration_ms: Rc::new(Cell::new(None)),
            transition_generation: Rc::new(Cell::new(0)),
            transition_duration_update_generation: Rc::new(Cell::new(0)),
            last_track_key: Rc::new(RefCell::new(None)),
        }
    }
}
