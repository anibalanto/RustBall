// ============================================================================
// SISTEMAS DE LÓGICA DE PARTIDO (HOST)
// ============================================================================
//
// Zonas del campo:
//   - Cancha interna   → juego normal
//   - Arco interior    → GOL al detectar pelota dentro
//   - Zona externa     → lateral o corner (si la pelota sale del campo)
//
// Flujo principal:
//   1. tick_match_time   → decrementa tiempo, detecta fin
//   2. detect_goal       → detecta gol via zonas del mapa
//   3. detect_ball_out   → detecta salida de campo (lateral/corner)
//   4. reset_ball_for_kickoff → recentra pelota tras gol
//   5. broadcast_match_state  → envía MatchUpdate periódicamente
// ============================================================================

use crate::host::host::{Ball, HostMatchGame, LoadedMap, NetworkSender, OutgoingMessage, Player, SetPieceLock, Sphere};
use bevy_rapier2d::dynamics::Dominance;
use crate::shared::match_game::{ball_in_goal, ball_out_of_bounds, MatchGame, MatchOps, SetPiece};
use crate::shared::protocol::{ControlMessage, GameConfig};
use bevy::prelude::*;
use bevy_rapier2d::prelude::*;

// ============================================================================
// HELPERS PRIVADOS
// ============================================================================

/// Difunde el estado actual del partido a todos los clientes (canal reliable).
pub fn broadcast_match_update(match_game: &MatchGame, network_tx: &NetworkSender) {
    let state = match_game.snapshot();
    let msg = ControlMessage::MatchUpdate(state);
    if let Ok(data) = bincode::serialize(&msg) {
        let _ = network_tx
            .0
            .send(OutgoingMessage::Broadcast { channel: 0, data });
    }
}

/// Calcula las semidimensiones del CAMPO INTERIOR (las líneas visibles, sin colisión).
///
/// En mapas estilo HaxBall hay dos zonas:
///   - Paredes exteriores (c_mask incluye "ball"): contienen la pelota físicamente.
///   - Líneas interiores del campo (c_mask vacío/sin "ball"): solo visuales.
///
/// "Fuera de campo" ocurre cuando la pelota cruza las líneas interiores hacia la zona
/// exterior pero antes de llegar a las paredes. Los vértices sin colisión con la pelota
/// marcan exactamente ese límite.
fn inner_field_half_dims(loaded_map: &LoadedMap, config: &GameConfig) -> (f32, f32) {
    if let Some(m) = &loaded_map.0 {
        // Vértices que NO colisionan con la pelota = líneas internas del campo
        let (max_x, max_y) = m
            .vertexes
            .iter()
            .filter(|v| {
                v.c_mask
                    .as_ref()
                    .map(|mask| !mask.iter().any(|s| s == "ball"))
                    .unwrap_or(true)
            })
            .fold((0.0f32, 0.0f32), |(mx, my), v| {
                (mx.max(v.x.abs()), my.max(v.y.abs()))
            });

        if max_x > 1.0 && max_y > 1.0 {
            return (max_x, max_y);
        }

        // Fallback: GameConfig (arena_width/2 = 4000 por defecto)
        (config.arena_width / 2.0, config.arena_height / 2.0)
    } else {
        (config.arena_width / 2.0, config.arena_height / 2.0)
    }
}

/// Devuelve true si la pelota está en la zona de acercamiento a algún arco,
/// es decir, pasó la línea interior del campo pero en el rango Y del arco.
/// En ese caso, se deja que `detect_goal` lo maneje al cruzar la línea del arco.
fn ball_in_goal_approach(
    ball_pos: Vec2,
    goals: &[crate::shared::map::Goal],
    inner_half_w: f32,
    ball_radius: f32,
) -> bool {
    for goal in goals {
        let dx = (goal.p1[0] - goal.p0[0]).abs();
        let dy = (goal.p1[1] - goal.p0[1]).abs();

        if dy >= dx {
            // Arco vertical (izquierda o derecha)
            let goal_x = (goal.p0[0] + goal.p1[0]) / 2.0;
            let y_min = goal.p0[1].min(goal.p1[1]) - ball_radius;
            let y_max = goal.p0[1].max(goal.p1[1]) + ball_radius;

            // ¿Pasó la línea interior y está en el rango Y del arco?
            let past_inner = if goal_x < 0.0 {
                ball_pos.x < -inner_half_w
            } else {
                ball_pos.x > inner_half_w
            };

            if past_inner && ball_pos.y >= y_min && ball_pos.y <= y_max {
                return true;
            }
        } else {
            // Arco horizontal (arriba o abajo)
            let goal_y = (goal.p0[1] + goal.p1[1]) / 2.0;
            let x_min = goal.p0[0].min(goal.p1[0]) - ball_radius;
            let x_max = goal.p0[0].max(goal.p1[0]) + ball_radius;

            let past_inner = if goal_y < 0.0 {
                ball_pos.y < -inner_half_w
            } else {
                ball_pos.y > inner_half_w
            };

            if past_inner && ball_pos.x >= x_min && ball_pos.x <= x_max {
                return true;
            }
        }
    }
    false
}

// ============================================================================
// SISTEMA: SEGUIMIENTO DE ÚLTIMO TOQUE Y REANUDACIÓN DE JUGADA PARADA
// ============================================================================

/// Registra qué equipo tocó la pelota por última vez (para lateral/corner).
/// También limpia `pending_set_piece` en cuanto la pelota empieza a moverse
/// tras ser colocada en el punto de reanudación.
pub fn update_ball_touch(
    config: Res<GameConfig>,
    player_query: Query<&Player>,
    sphere_query: Query<&Transform, (With<Sphere>, Without<Ball>)>,
    ball_query: Query<(&Transform, &Velocity), With<Ball>>,
    mut match_game: ResMut<HostMatchGame>,
) {
    if !match_game.0.is_running() {
        return;
    }

    let Ok((ball_transform, ball_vel)) = ball_query.single() else {
        return;
    };
    let ball_pos = ball_transform.translation.truncate();

    // Si hay una jugada a balón parado pendiente:
    // - Durante la fase de movimiento libre (timer activo): no tocar nada.
    // - Una vez colocada/congelada (timer = None): limpiar cuando la pelota se mueva.
    if match_game.0.pending_set_piece.is_some() {
        if match_game.0.set_piece_delay_timer.is_none() && ball_vel.linvel.length() > 30.0 {
            match_game.0.pending_set_piece = None;
        }
        return;
    }

    // Detectar qué jugador está en contacto con la pelota
    let contact_radius = config.sphere_radius + config.ball_radius + 5.0;

    for player in player_query.iter() {
        let Ok(sphere_transform) = sphere_query.get(player.sphere) else {
            continue;
        };
        let diff = ball_pos - sphere_transform.translation.truncate();
        if diff.length() < contact_radius {
            match_game.0.record_touch(player.team_index);
            break;
        }
    }
}

// ============================================================================
// SISTEMA: TICK DEL TIEMPO
// ============================================================================

/// Avanza el reloj del partido. Emite broadcast cuando cambia el estado.
pub fn tick_match_time(
    time: Res<Time>,
    mut match_game: ResMut<HostMatchGame>,
    network_tx: Res<NetworkSender>,
) {
    if match_game.0.is_waiting() {
        return;
    }

    let status_before = match_game.0.status.clone();
    match_game.0.tick(time.delta_secs());

    // Emitir broadcast si el estado cambió (gol reanudado, partido terminado)
    if match_game.0.status != status_before {
        broadcast_match_update(&match_game.0, &network_tx);
    }
}

// ============================================================================
// SISTEMA: DETECCIÓN DE GOL
// ============================================================================

/// Detecta si la pelota entró en la parte interna de algún arco.
///
/// Asignación de equipo que anota:
///   - "red"  en `goal.team` → el equipo rojo (team 0) defiende → team 1 anota
///   - "blue" en `goal.team` → el equipo azul (team 1) defiende → team 0 anota
pub fn detect_goal(
    ball_query: Query<&Transform, With<Ball>>,
    loaded_map: Res<LoadedMap>,
    config: Res<GameConfig>,
    mut match_game: ResMut<HostMatchGame>,
    network_tx: Res<NetworkSender>,
) {
    // Solo detectar goles cuando el partido está en curso, no hay kickoff pendiente
    // y no hay un set piece en curso (pelota fuera de juego).
    if !match_game.0.is_running() || match_game.0.pending_kickoff || match_game.0.pending_set_piece.is_some() {
        return;
    }

    let ball_transform = match ball_query.single() {
        Ok(t) => t,
        Err(_) => return,
    };
    let ball_pos = Vec2::new(
        ball_transform.translation.x,
        ball_transform.translation.y,
    );

    let map = match &loaded_map.0 {
        Some(m) => m,
        None => return,
    };

    for goal in &map.goals {
        let scoring_team: u8 = match goal.team.as_str() {
            "red" => 1,  // equipo rojo defiende → azul (1) anota
            "blue" => 0, // equipo azul defiende → rojo (0) anota
            _ => continue,
        };

        if ball_in_goal(ball_pos, goal.p0, goal.p1, config.ball_radius) {
            match_game.0.score_goal(scoring_team);
            broadcast_match_update(&match_game.0, &network_tx);
            return; // Un solo gol por frame
        }
    }
}

// ============================================================================
// SISTEMA: DETECCIÓN DE PELOTA FUERA DE CAMPO
// ============================================================================

/// Detecta si la pelota salió de los límites del campo, la reposiciona en el
/// punto de reanudación correcto y la congela.
///
/// Tipos de reanudación:
///   - Salida por la banda   → lateral (equipo contrario al último toque)
///   - Salida por el fondo   → corner (defensor tocó último) o saque de puerta (atacante tocó)
pub fn detect_ball_out(
    ball_query: Query<&Transform, With<Ball>>,
    loaded_map: Res<LoadedMap>,
    config: Res<GameConfig>,
    mut match_game: ResMut<HostMatchGame>,
    network_tx: Res<NetworkSender>,
) {
    // Solo actuar si el partido corre y no hay ya una jugada a balón parado pendiente
    if !match_game.0.is_running() || match_game.0.pending_set_piece.is_some() {
        return;
    }

    let Ok(ball_transform) = ball_query.single() else {
        return;
    };
    let ball_pos = Vec2::new(ball_transform.translation.x, ball_transform.translation.y);

    // Usar las dimensiones del campo INTERIOR (vértices sin colisión con pelota)
    let (half_w, half_h) = inner_field_half_dims(&loaded_map, &config);

    if !ball_out_of_bounds(ball_pos, half_w, half_h) {
        return;
    }

    // Ignorar si la pelota está en la zona de aproximación al arco
    // (entre la línea interior y la línea del arco) — lo maneja detect_goal
    if let Some(map) = &loaded_map.0 {
        if ball_in_goal_approach(ball_pos, &map.goals, half_w, config.ball_radius) {
            return;
        }
    }

    // Margen para que la posición de reanudación quede dentro del campo
    let margin = config.ball_radius * 2.0 + 1.0;

    let last_touch = match_game.0.last_team_touch;
    let is_sideline = ball_pos.y.abs() > half_h;

    let set_piece = if is_sideline {
        // ── Lateral ──────────────────────────────────────────────────────────
        let sacking_team = last_touch.map(|t| 1 - t).unwrap_or(0);
        let pos = Vec2::new(
            ball_pos.x.clamp(-(half_w - margin), half_w - margin),
            ball_pos.y.signum() * (half_h - margin),
        );
        println!(
            "🚩 Lateral — equipo {} saca en ({:.0}, {:.0})",
            sacking_team, pos.x, pos.y
        );
        SetPiece::Lateral { team: sacking_team, position: pos }
    } else {
        // ── Fondo (corner o saque de puerta) ─────────────────────────────────
        let x_sign = ball_pos.x.signum();
        // El equipo defensor es el que defiende ese extremo
        let defending_team = if x_sign < 0.0 { 0u8 } else { 1u8 };

        if last_touch == Some(defending_team) {
            // El defensor tocó la pelota → corner para el atacante
            let attacker = 1 - defending_team;
            let pos = Vec2::new(
                x_sign * (half_w - margin),
                ball_pos.y.signum() * (half_h - margin),
            );
            println!(
                "🚩 Corner — equipo {} saca en ({:.0}, {:.0})",
                attacker, pos.x, pos.y
            );
            SetPiece::Corner { team: attacker, position: pos }
        } else {
            // El atacante tocó la pelota → saque de puerta para el defensor
            let pos = Vec2::new(x_sign * (half_w - margin * 4.0), 0.0);
            println!(
                "🚩 Saque de puerta — equipo {} saca en ({:.0}, {:.0})",
                defending_team, pos.x, pos.y
            );
            SetPiece::GoalKick { team: defending_team, position: pos }
        }
    };

    // Registrar el set piece y arrancar el timer de movimiento libre.
    // La pelota se deja rodar naturalmente durante `set_piece_delay_secs`;
    // el sistema `apply_set_piece_position` la teleportará y congelará al expirar.
    match_game.0.last_team_touch = None;
    match_game.0.pending_set_piece = Some(set_piece);
    match_game.0.set_piece_delay_timer = Some(config.set_piece_delay_secs);
    broadcast_match_update(&match_game.0, &network_tx);
}

// ============================================================================
// SISTEMA: RESET DE PELOTA TRAS GOL (SAQUE DE CENTRO)
// ============================================================================

/// Recentra la pelota y detiene su movimiento cuando hay un saque de centro
/// pendiente (al inicio del partido o después de un gol).
pub fn reset_ball_for_kickoff(
    mut match_game: ResMut<HostMatchGame>,
    mut ball_query: Query<(&mut Transform, &mut Velocity), With<Ball>>,
    network_tx: Res<NetworkSender>,
) {
    if !match_game.0.pending_kickoff {
        return;
    }
    match_game.0.pending_kickoff = false;

    let Ok((mut transform, mut velocity)) = ball_query.single_mut() else {
        return;
    };

    transform.translation.x = 0.0;
    transform.translation.y = 0.0;
    velocity.linvel = Vec2::ZERO;
    velocity.angvel = 0.0;

    println!("🔄 Saque de centro — pelota recentrada");
    broadcast_match_update(&match_game.0, &network_tx);
}

// ============================================================================
// SISTEMA: TELEPORT DE PELOTA AL PUNTO DE REANUDACIÓN (TRAS DELAY)
// ============================================================================

/// Cuenta regresiva del timer de set piece. Cuando expira, teleporta la pelota
/// al punto de reanudación y la congela, activando la zona de exclusión.
pub fn apply_set_piece_position(
    time: Res<Time>,
    mut match_game: ResMut<HostMatchGame>,
    mut ball_query: Query<(&mut Transform, &mut Velocity), With<Ball>>,
    network_tx: Res<NetworkSender>,
) {
    let Some(timer) = match_game.0.set_piece_delay_timer else {
        return;
    };

    let new_timer = timer - time.delta_secs();
    if new_timer > 0.0 {
        match_game.0.set_piece_delay_timer = Some(new_timer);
        return;
    }

    // Timer expirado: teleportar y congelar la pelota
    match_game.0.set_piece_delay_timer = None;

    let restart_pos = match &match_game.0.pending_set_piece {
        Some(sp) => sp.position().unwrap_or(Vec2::ZERO),
        None => return,
    };

    let Ok((mut transform, mut velocity)) = ball_query.single_mut() else {
        return;
    };
    transform.translation.x = restart_pos.x;
    transform.translation.y = restart_pos.y;
    velocity.linvel = Vec2::ZERO;
    velocity.angvel = 0.0;

    println!("📍 Set piece colocada en ({:.0}, {:.0})", restart_pos.x, restart_pos.y);
    broadcast_match_update(&match_game.0, &network_tx);
}

// ============================================================================
// SISTEMA: ZONA DE EXCLUSIÓN DURANTE JUGADA A BALÓN PARADO
// ============================================================================

/// Empuja a los jugadores del equipo contrario fuera del radio de exclusión
/// mientras la pelota está colocada en el punto de reanudación.
pub fn enforce_set_piece_exclusion(
    match_game: Res<HostMatchGame>,
    config: Res<GameConfig>,
    player_query: Query<&Player>,
    mut sphere_query: Query<(&Transform, &mut Velocity), (With<Sphere>, Without<Ball>)>,
) {
    // Solo aplica cuando la pelota ya está colocada (timer expirado)
    if match_game.0.set_piece_delay_timer.is_some() {
        return;
    }

    let Some(ref set_piece) = match_game.0.pending_set_piece else {
        return;
    };

    let Some(set_piece_pos) = set_piece.position() else {
        return; // KickOff no tiene zona de exclusión
    };
    let kicking_team = set_piece.team();
    let exclusion_radius = config.set_piece_exclusion_radius + config.sphere_radius;

    for player in player_query.iter() {
        if player.team_index == kicking_team {
            continue; // El equipo que saca puede acercarse libremente
        }

        let Ok((sphere_transform, mut velocity)) = sphere_query.get_mut(player.sphere) else {
            continue;
        };

        let player_pos = sphere_transform.translation.truncate();
        let diff = player_pos - set_piece_pos;
        let dist = diff.length();

        if dist < exclusion_radius {
            let push_dir = if dist > 1.0 { diff.normalize() } else { Vec2::Y };
            let penetration = exclusion_radius - dist;
            // Impulso proporcional a la penetración, evitando acumulación indefinida
            let push = push_dir * penetration * 25.0;
            velocity.linvel += push;
            // Limitar a velocidad caminando * 2 para no lanzar al jugador
            let max_speed = config.player_speed_walking * 2.0;
            if velocity.linvel.length() > max_speed {
                velocity.linvel = velocity.linvel.normalize() * max_speed;
            }
        }
    }
}

// ============================================================================
// SISTEMA: LOCK DE PELOTA DURANTE JUGADA A BALÓN PARADO
// ============================================================================

/// Actualiza `SetPieceLock` y la `Dominance` de la pelota.
///
/// Cuando la pelota está colocada en el punto de reanudación:
///   - `SetPieceLock(true)` → engine.rs omite los impulsos suaves (push, attract, dash).
///   - `Dominance::group(127)` → Rapier trata la pelota como masa infinita en contactos.
///     Los jugadores rebotan contra ella sin moverla. Los kicks (ExternalImpulse)
///     siguen funcionando porque bypasean el solver de contacto.
pub fn update_set_piece_lock(
    match_game: Res<HostMatchGame>,
    mut lock: ResMut<SetPieceLock>,
    mut ball_query: Query<&mut Dominance, With<Ball>>,
) {
    let locked = match_game.0.pending_set_piece.is_some()
        && match_game.0.set_piece_delay_timer.is_none();

    if lock.0 == locked {
        return; // Sin cambio, no tocar Dominance
    }
    lock.0 = locked;

    let Ok(mut dominance) = ball_query.single_mut() else {
        return;
    };
    dominance.groups = if locked { 127 } else { 0 };
}

// ============================================================================
// SISTEMA: BROADCAST PERIÓDICO DEL ESTADO DEL PARTIDO
// ============================================================================

/// Difunde el estado del partido a todos los clientes una vez por segundo
/// para mantener sincronizado el tiempo restante.
pub fn broadcast_match_state(
    time: Res<Time>,
    mut timer: Local<f32>,
    match_game: Res<HostMatchGame>,
    network_tx: Res<NetworkSender>,
) {
    // No hay nada que difundir si el partido no empezó
    if match_game.0.is_waiting() {
        return;
    }

    *timer += time.delta_secs();
    if *timer >= 1.0 {
        *timer = 0.0;
        broadcast_match_update(&match_game.0, &network_tx);
    }
}
