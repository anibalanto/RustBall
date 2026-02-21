/// Módulo de abstracción de input para el juego.
/// Permite recibir input de keyboard, joystick o red sin dependencias directas.

/// Acciones del juego que pueden ser mapeadas a diferentes fuentes de input
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameAction {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Kick,
    StopInteract,
    Dash,
    Sprint, // Correr
    Mode,   // Modo cubo (toggle)
}

/// Trait que debe implementar cualquier fuente de input
pub trait InputSource: Send + Sync {
    /// Retorna true si la acción está siendo presionada en este frame
    fn is_pressed(&self, action: GameAction) -> bool;

    /// Retorna true si la acción fue presionada en este frame (just_pressed)
    fn just_pressed(&self, action: GameAction) -> bool;

    /// Retorna true si la acción fue soltada en este frame (just_released)
    fn just_released(&self, action: GameAction) -> bool;

    /// Retorna la dirección de comba en espacio absoluto.
    /// Fallback = sin comba. NetworkInputSource sobreescribe con el valor real del PlayerInput.
    fn get_curve_dir(&self) -> bevy::math::Vec2 {
        bevy::math::Vec2::ZERO
    }

    /// Actualiza el estado interno (llamar una vez por frame)
    #[allow(dead_code)]
    fn update(&mut self);
}
