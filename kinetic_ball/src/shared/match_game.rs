use bevy::prelude::Vec2;
use serde::{Deserialize, Serialize};

// ============================================================================
// ZONAS DEL CAMPO
// ============================================================================

/// Zona donde se encuentra la pelota en el campo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldZone {
    /// Parte interna de la cancha (juego normal)
    InnerField,
    /// Parte interna del arco de un equipo (team = equipo que defiende)
    GoalInterior { defending_team: u8 },
    /// Fuera del campo (lateral o corner)
    OutOfBounds { is_sideline: bool },
}

// ============================================================================
// ESTADO DEL PARTIDO
// ============================================================================

/// Estado actual del partido
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatchStatus {
    /// Esperando que el admin inicie el partido
    WaitingToStart,
    /// Partido en curso
    Running,
    /// Pausa breve después de un gol antes del saque de centro
    GoalScored { scoring_team: u8, countdown: f32 },
    /// Partido terminado
    Ended,
}

/// Ganador del partido
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatchWinner {
    Team(u8),
    Draw,
}

/// Tipo de reanudación de juego
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SetPiece {
    /// Saque de banda: equipo que saca, posición en la banda
    Lateral { team: u8, position: Vec2 },
    /// Tiro de esquina: equipo que saca, esquina más cercana
    Corner { team: u8, position: Vec2 },
    /// Saque de puerta: equipo que saca, posición del arco
    GoalKick { team: u8, position: Vec2 },
    /// Saque de centro (inicio o tras gol)
    KickOff,
}

impl SetPiece {
    /// Posición del punto de reanudación. `None` para KickOff (siempre en centro).
    pub fn position(&self) -> Option<Vec2> {
        match self {
            SetPiece::Lateral { position, .. } => Some(*position),
            SetPiece::Corner { position, .. } => Some(*position),
            SetPiece::GoalKick { position, .. } => Some(*position),
            SetPiece::KickOff => None,
        }
    }

    /// Equipo que tiene el saque.
    pub fn team(&self) -> u8 {
        match self {
            SetPiece::Lateral { team, .. } => *team,
            SetPiece::Corner { team, .. } => *team,
            SetPiece::GoalKick { team, .. } => *team,
            SetPiece::KickOff => 0,
        }
    }
}

// ============================================================================
// STRUCT PRINCIPAL
// ============================================================================

/// Estado completo de un partido
#[derive(Debug, Clone, Serialize, Deserialize, bevy::prelude::Resource)]
pub struct MatchGame {
    /// Marcador [equipo_0, equipo_1]
    pub score: [u8; 2],
    /// Tiempo restante en segundos
    pub time_remaining: f32,
    /// Duración total configurada en segundos
    pub duration: f32,
    /// Estado del partido
    pub status: MatchStatus,
    /// Último equipo que tocó la pelota (para determinar lateral/corner)
    pub last_team_touch: Option<u8>,
    /// Saque de centro pendiente (tras gol o inicio)
    pub pending_kickoff: bool,
    /// Set piece pendiente
    pub pending_set_piece: Option<SetPiece>,
    /// Cuenta regresiva antes de teleportar la pelota al punto de reanudación.
    /// `Some(t)` → pelota en movimiento libre, `None` → pelota ya colocada/congelada.
    pub set_piece_delay_timer: Option<f32>,
}

/// Snapshot serializable del partido para enviar por red
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchGameState {
    pub score: [u8; 2],
    pub time_remaining: f32,
    pub status: MatchStatus,
    pub winner: Option<MatchWinner>,
    /// Jugada a balón parado activa (solo presente cuando la pelota ya está
    /// colocada en el punto de reanudación, no durante el movimiento libre).
    pub pending_set_piece: Option<SetPiece>,
}

impl From<&MatchGame> for MatchGameState {
    fn from(g: &MatchGame) -> Self {
        Self {
            score: g.score,
            time_remaining: g.time_remaining,
            status: g.status.clone(),
            winner: g.winner(),
            // Solo incluir el set piece cuando la pelota ya está colocada (timer expirado)
            pending_set_piece: if g.set_piece_delay_timer.is_none() {
                g.pending_set_piece.clone()
            } else {
                None
            },
        }
    }
}

// ============================================================================
// TRAIT
// ============================================================================

/// Operaciones del partido
pub trait MatchOps {
    /// Crea un partido nuevo con la duración indicada (en segundos)
    fn new(duration_secs: f32) -> Self;

    /// Inicia el partido (pasa de WaitingToStart → Running)
    fn start(&mut self);

    /// Avanza el tiempo del partido por `delta_secs` segundos.
    /// Actualiza contadores internos y cambia estado si corresponde.
    fn tick(&mut self, delta_secs: f32);

    /// Registra un gol del equipo `scoring_team` (0 o 1)
    fn score_goal(&mut self, scoring_team: u8);

    /// Registra que el equipo `team` tocó la pelota por última vez
    fn record_touch(&mut self, team: u8);

    /// Calcula y retorna el ganador. Sólo válido cuando el estado es Ended.
    fn winner(&self) -> Option<MatchWinner>;

    /// Finaliza el partido anticipadamente y retorna el ganador
    fn finalize(&mut self) -> MatchWinner;

    /// Retorna `true` si el partido está en espera de inicio
    fn is_waiting(&self) -> bool;

    /// Retorna `true` si el partido está corriendo normalmente
    fn is_running(&self) -> bool;

    /// Retorna `true` si estamos en la pausa tras un gol
    fn is_goal_countdown(&self) -> bool;

    /// Retorna `true` si el partido terminó
    fn is_ended(&self) -> bool;

    /// Snapshot del estado para enviar por red
    fn snapshot(&self) -> MatchGameState;
}

// ============================================================================
// IMPLEMENTACIÓN
// ============================================================================

impl MatchOps for MatchGame {
    fn new(duration_secs: f32) -> Self {
        Self {
            score: [0; 2],
            time_remaining: duration_secs,
            duration: duration_secs,
            status: MatchStatus::WaitingToStart,
            last_team_touch: None,
            pending_kickoff: true,
            pending_set_piece: None,
            set_piece_delay_timer: None,
        }
    }

    fn start(&mut self) {
        if matches!(self.status, MatchStatus::WaitingToStart) {
            self.status = MatchStatus::Running;
            self.pending_kickoff = true;
            println!(
                "🏆 Partido iniciado — duración: {}s",
                self.duration
            );
        }
    }

    fn tick(&mut self, delta_secs: f32) {
        // Clonar para evitar borrow doble sobre self.status
        let current_status = self.status.clone();
        match current_status {
            MatchStatus::Running => {
                self.time_remaining -= delta_secs;
                if self.time_remaining <= 0.0 {
                    self.time_remaining = 0.0;
                    self.status = MatchStatus::Ended;
                    println!(
                        "⏱️  Tiempo terminado — {}-{}",
                        self.score[0], self.score[1]
                    );
                }
            }
            MatchStatus::GoalScored { scoring_team, countdown } => {
                let new_countdown = countdown - delta_secs;
                if new_countdown <= 0.0 {
                    self.status = MatchStatus::Running;
                    self.pending_kickoff = true;
                    self.last_team_touch = None;
                    println!("▶️  Reanudando partido después de gol");
                } else {
                    self.status = MatchStatus::GoalScored {
                        scoring_team,
                        countdown: new_countdown,
                    };
                }
            }
            _ => {}
        }
    }

    fn score_goal(&mut self, scoring_team: u8) {
        if !self.is_running() {
            return;
        }
        let team_idx = scoring_team as usize;
        self.score[team_idx] += 1;
        self.status = MatchStatus::GoalScored {
            scoring_team,
            countdown: 3.0,
        };
        self.last_team_touch = None;
        println!(
            "⚽ ¡GOL! Equipo {} anota — marcador: {}-{}",
            scoring_team, self.score[0], self.score[1]
        );
    }

    fn record_touch(&mut self, team: u8) {
        self.last_team_touch = Some(team);
    }

    fn winner(&self) -> Option<MatchWinner> {
        if !matches!(self.status, MatchStatus::Ended) {
            return None;
        }
        match self.score[0].cmp(&self.score[1]) {
            std::cmp::Ordering::Greater => Some(MatchWinner::Team(0)),
            std::cmp::Ordering::Less => Some(MatchWinner::Team(1)),
            std::cmp::Ordering::Equal => Some(MatchWinner::Draw),
        }
    }

    fn finalize(&mut self) -> MatchWinner {
        self.status = MatchStatus::Ended;
        self.winner().unwrap_or(MatchWinner::Draw)
    }

    fn is_waiting(&self) -> bool {
        matches!(self.status, MatchStatus::WaitingToStart)
    }

    fn is_running(&self) -> bool {
        matches!(self.status, MatchStatus::Running)
    }

    fn is_goal_countdown(&self) -> bool {
        matches!(self.status, MatchStatus::GoalScored { .. })
    }

    fn is_ended(&self) -> bool {
        matches!(self.status, MatchStatus::Ended)
    }

    fn snapshot(&self) -> MatchGameState {
        MatchGameState::from(self)
    }
}

impl Default for MatchGame {
    fn default() -> Self {
        Self::new(0.0)
    }
}

// ============================================================================
// UTILIDADES DE DETECCIÓN DE ZONA
// ============================================================================

/// Determina si la pelota está dentro del arco.
/// Soporta arcos verticales (p0.x ≈ p1.x) y horizontales (p0.y ≈ p1.y).
pub fn ball_in_goal(
    ball_pos: Vec2,
    p0: [f32; 2],
    p1: [f32; 2],
    ball_radius: f32,
) -> bool {
    let dx = (p1[0] - p0[0]).abs();
    let dy = (p1[1] - p0[1]).abs();

    if dy >= dx {
        // Arco vertical (izquierda o derecha)
        let goal_x = (p0[0] + p1[0]) / 2.0;
        let y_min = p0[1].min(p1[1]) - ball_radius;
        let y_max = p0[1].max(p1[1]) + ball_radius;

        let inside_x = if goal_x <= 0.0 {
            ball_pos.x < goal_x
        } else {
            ball_pos.x > goal_x
        };

        inside_x && ball_pos.y >= y_min && ball_pos.y <= y_max
    } else {
        // Arco horizontal (arriba o abajo)
        let goal_y = (p0[1] + p1[1]) / 2.0;
        let x_min = p0[0].min(p1[0]) - ball_radius;
        let x_max = p0[0].max(p1[0]) + ball_radius;

        let inside_y = if goal_y <= 0.0 {
            ball_pos.y < goal_y
        } else {
            ball_pos.y > goal_y
        };

        inside_y && ball_pos.x >= x_min && ball_pos.x <= x_max
    }
}

/// Determina si la pelota está fuera del campo según sus límites.
/// `half_w` y `half_h` son las semidimensiones del campo (desde el centro).
pub fn ball_out_of_bounds(ball_pos: Vec2, half_w: f32, half_h: f32) -> bool {
    ball_pos.x.abs() > half_w || ball_pos.y.abs() > half_h
}

/// Determina la zona del campo donde se encuentra la pelota.
pub fn get_field_zone(
    ball_pos: Vec2,
    goals: &[crate::shared::map::Goal],
    half_w: f32,
    half_h: f32,
    ball_radius: f32,
) -> FieldZone {
    // Primero verificar si está en algún arco
    for goal in goals {
        let defending_team = match goal.team.as_str() {
            "red" => 0u8,
            "blue" => 1u8,
            _ => continue,
        };
        if ball_in_goal(ball_pos, goal.p0, goal.p1, ball_radius) {
            return FieldZone::GoalInterior { defending_team };
        }
    }

    // Verificar si está fuera de los límites del campo
    if ball_out_of_bounds(ball_pos, half_w, half_h) {
        let is_sideline = ball_pos.y.abs() > half_h;
        return FieldZone::OutOfBounds { is_sideline };
    }

    FieldZone::InnerField
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_match_waiting() {
        let m = MatchGame::new(90.0);
        assert!(m.is_waiting());
        assert_eq!(m.time_remaining, 90.0);
        assert_eq!(m.score, [0, 0]);
    }

    #[test]
    fn test_start_match() {
        let mut m = MatchGame::new(90.0);
        m.start();
        assert!(m.is_running());
    }

    #[test]
    fn test_score_goal() {
        let mut m = MatchGame::new(90.0);
        m.start();
        m.score_goal(1);
        assert!(m.is_goal_countdown());
        assert_eq!(m.score[1], 1);
    }

    #[test]
    fn test_tick_ends_match() {
        let mut m = MatchGame::new(1.0);
        m.start();
        m.tick(2.0);
        assert!(m.is_ended());
        assert_eq!(m.time_remaining, 0.0);
    }

    #[test]
    fn test_goal_countdown_resumes() {
        let mut m = MatchGame::new(90.0);
        m.start();
        m.score_goal(0);
        m.tick(3.5); // Supera el countdown de 3 segundos
        assert!(m.is_running());
        assert!(m.pending_kickoff);
    }

    #[test]
    fn test_winner_team0() {
        let mut m = MatchGame::new(1.0);
        m.start();
        m.score_goal(0);
        m.tick(4.0); // Supera countdown
        m.tick(1.0); // Tiempo termina
        assert_eq!(m.winner(), Some(MatchWinner::Team(0)));
    }

    #[test]
    fn test_winner_draw() {
        let mut m = MatchGame::new(1.0);
        m.start();
        m.tick(2.0);
        assert_eq!(m.winner(), Some(MatchWinner::Draw));
    }

    #[test]
    fn test_ball_in_vertical_goal_left() {
        // Arco izquierdo: línea en x=-700, y de -64 a 64
        let inside = ball_in_goal(Vec2::new(-750.0, 0.0), [-700.0, -64.0], [-700.0, 64.0], 15.0);
        assert!(inside);
        let outside = ball_in_goal(Vec2::new(-600.0, 0.0), [-700.0, -64.0], [-700.0, 64.0], 15.0);
        assert!(!outside);
    }

    #[test]
    fn test_ball_in_vertical_goal_right() {
        // Arco derecho: línea en x=700, y de -64 a 64
        let inside = ball_in_goal(Vec2::new(750.0, 0.0), [700.0, -64.0], [700.0, 64.0], 15.0);
        assert!(inside);
    }

    #[test]
    fn test_get_field_zone_inner() {
        let goals = vec![];
        let zone = get_field_zone(Vec2::new(0.0, 0.0), &goals, 700.0, 370.0, 15.0);
        assert_eq!(zone, FieldZone::InnerField);
    }

    #[test]
    fn test_get_field_zone_out_of_bounds() {
        let goals = vec![];
        let zone = get_field_zone(Vec2::new(0.0, 400.0), &goals, 700.0, 370.0, 15.0);
        assert_eq!(zone, FieldZone::OutOfBounds { is_sideline: true });
    }
}
