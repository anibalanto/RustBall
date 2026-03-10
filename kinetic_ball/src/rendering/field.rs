use bevy::camera::ScalingMode;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::sprite_render::ColorMaterial;

use crate::components::{
    DefaultFieldLine, FieldBackground, InGameEntity, MapLineEntity, MinimapCamera,
    MinimapFieldBackground, MinimapFieldLine, SetPieceCircle,
};
use crate::rendering::minimap::spawn_minimap_lines;
use crate::resources::{ClientMatchGame, LoadedMap};
use crate::shared::map::Map;
use crate::shared::match_game::SetPiece;
use crate::shared::protocol::GameConfig;

// Constante Z para las líneas del mapa (entre el piso Z=0 y los jugadores Z=10+)
pub const MAP_LINES_Z: f32 = 5.0;
/// Grosor para paredes externas y estructuras de arco
pub const LINE_THICKNESS: f32 = 3.0;
/// Grosor para las líneas divisorias interiores del campo (línea de medio, áreas, etc.)
pub const FIELD_MARKING_THICKNESS: f32 = 15.0;

/// Convierte un color hexadecimal de mapa (ej: "ff4444") a `Color`.
fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
    Some(Color::srgb(r, g, b))
}

// Sistema para ocultar líneas por defecto, ajustar campo y crear líneas del mapa
pub fn adjust_field_for_map(
    mut commands: Commands,
    loaded_map: Res<LoadedMap>,
    mut default_lines: Query<&mut Visibility, With<DefaultFieldLine>>,
    mut field_bg: Query<
        (&mut Sprite, &mut Transform),
        (
            With<FieldBackground>,
            Without<DefaultFieldLine>,
            Without<MinimapFieldBackground>,
        ),
    >,
    mut minimap_bg: Query<&mut Sprite, (With<MinimapFieldBackground>, Without<FieldBackground>)>,
    mut minimap_camera: Query<&mut Projection, With<MinimapCamera>>,
    map_lines: Query<Entity, With<MapLineEntity>>,
    minimap_lines: Query<Entity, With<MinimapFieldLine>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if loaded_map.is_changed() {
        // Eliminar líneas del mapa anterior
        for entity in map_lines.iter() {
            commands.entity(entity).despawn();
        }
        // Eliminar líneas del minimapa anterior
        for entity in minimap_lines.iter() {
            commands.entity(entity).despawn();
        }

        if let Some(map) = &loaded_map.0 {
            // Hay mapa: ocultar líneas por defecto
            for mut visibility in default_lines.iter_mut() {
                *visibility = Visibility::Hidden;
            }

            // Ajustar tamaño del campo según dimensiones del mapa
            let width = map.width.or(map.bg.width);
            let height = map.height.or(map.bg.height);

            if let (Some(w), Some(h)) = (width, height) {
                // Campo principal
                if let Ok((mut sprite, _transform)) = field_bg.single_mut() {
                    sprite.custom_size = Some(Vec2::new(w, h));
                    println!("🎨 Campo ajustado a dimensiones del mapa: {}x{}", w, h);
                }
                // Fondo del minimapa
                if let Ok(mut minimap_sprite) = minimap_bg.single_mut() {
                    minimap_sprite.custom_size = Some(Vec2::new(w, h));
                }
                // Proyección de la cámara del minimapa
                // Ajustar para que el mapa llene el minimapa (300x180)
                if let Ok(mut projection) = minimap_camera.single_mut() {
                    let minimap_aspect = 300.0 / 180.0; // aspect ratio del minimapa
                    let map_aspect = w / h;
                    let zoom = 1.0; // campo completo

                    let (cam_w, cam_h) = if map_aspect > minimap_aspect {
                        // Mapa más ancho: el ancho define la escala
                        (w * zoom, w / minimap_aspect * zoom)
                    } else {
                        // Mapa más alto: la altura define la escala
                        (h * minimap_aspect * zoom, h * zoom)
                    };

                    *projection = Projection::Orthographic(OrthographicProjection {
                        scaling_mode: ScalingMode::Fixed {
                            width: cam_w,
                            height: cam_h,
                        },
                        ..OrthographicProjection::default_2d()
                    });
                    println!("🗺️  Cámara minimapa ajustada a: {}x{}", cam_w, cam_h);
                }
            } else {
                println!("⚠️  Mapa sin dimensiones definidas, usando tamaño por defecto");
            }

            // Crear líneas del mapa como sprites
            spawn_map_lines(&mut commands, map, &mut meshes, &mut materials);
            // Crear líneas del minimapa
            spawn_minimap_lines(&mut commands, map);
        } else {
            // No hay mapa: mostrar líneas por defecto
            for mut visibility in default_lines.iter_mut() {
                *visibility = Visibility::Visible;
            }
        }
    }
}

// Crea sprites para las líneas del mapa
pub fn spawn_map_lines(
    commands: &mut Commands,
    map: &Map,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
) {
    println!(
        "🗺️  spawn_map_lines: {} vértices, {} segmentos, {} discos",
        map.vertexes.len(),
        map.segments.len(),
        map.discs.len()
    );

    // Colores según tipo de interacción
    let ball_color = Color::srgb(0.3, 0.7, 1.0); // Azul claro - solo pelota
    let player_color = Color::srgb(0.3, 1.0, 0.5); // Verde claro - solo jugadores
    let decorative_color = Color::WHITE; // Blanco - líneas divisorias del campo

    // Dibujar segmentos (líneas)
    for segment in &map.segments {
        if !segment.is_visible() {
            continue;
        }

        if segment.v0 >= map.vertexes.len() || segment.v1 >= map.vertexes.len() {
            continue;
        }

        let v0 = &map.vertexes[segment.v0];
        let v1 = &map.vertexes[segment.v1];

        let p0 = Vec2::new(v0.x, v0.y);
        let p1 = Vec2::new(v1.x, v1.y);

        // Determinar si es línea decorativa (sin colisión) o física
        let is_decorative = segment.c_mask.as_ref().map_or(true, |m| {
            m.is_empty() || m.iter().all(|s| s.is_empty())
        });

        // Determinar color según cMask (o color del mapa si decorativa)
        let line_color = if is_decorative {
            // Usar color definido en el mapa, si existe
            segment
                .color
                .as_deref()
                .and_then(parse_hex_color)
                .unwrap_or(decorative_color)
        } else if let Some(cmask) = &segment.c_mask {
            if cmask.iter().any(|m| m == "ball")
                && !cmask.iter().any(|m| m == "red" || m == "blue")
            {
                ball_color
            } else {
                player_color
            }
        } else {
            decorative_color
        };

        // Las líneas divisorias interiores son más gruesas que las paredes
        let thickness = if is_decorative { FIELD_MARKING_THICKNESS } else { LINE_THICKNESS };

        let curve_factor = segment.curve.or(segment.curve_f).unwrap_or(0.0);

        if curve_factor.abs() < 0.01 {
            spawn_line_segment(commands, p0, p1, line_color, thickness);
        } else {
            let points = approximate_curve_for_rendering(p0, p1, curve_factor, 24);
            for i in 0..points.len() - 1 {
                spawn_line_segment(commands, points[i], points[i + 1], line_color, thickness);
            }
        }
    }

    // Dibujar palos del arco: anillo de color de equipo + relleno blanco.
    // El equipo se deduce por el lado del campo: X < 0 → rojo, X > 0 → azul.
    for disc in &map.discs {
        let pos = Vec2::new(disc.pos[0], disc.pos[1]);
        let team_color = if disc.pos[0] < 0.0 {
            Color::srgb(1.0, 0.25, 0.25) // rojo
        } else {
            Color::srgb(0.25, 0.45, 1.0) // azul
        };
        // Anillo exterior con el color del equipo
        spawn_circle(commands, meshes, materials, pos, disc.radius + 8.0, team_color);
        // Relleno blanco (el poste)
        spawn_circle(commands, meshes, materials, pos, disc.radius, Color::WHITE);
    }
}

// Crea un sprite rectangular para representar una línea
pub fn spawn_line_segment(commands: &mut Commands, p0: Vec2, p1: Vec2, color: Color, thickness: f32) {
    let delta = p1 - p0;
    let length = delta.length();
    if length < 0.01 {
        return;
    }

    let midpoint = (p0 + p1) * 0.5;
    let angle = delta.y.atan2(delta.x);

    commands.spawn((
        InGameEntity,
        Sprite {
            color,
            custom_size: Some(Vec2::new(length, thickness)),
            ..default()
        },
        Transform::from_xyz(midpoint.x, midpoint.y, MAP_LINES_Z)
            .with_rotation(Quat::from_rotation_z(angle)),
        MapLineEntity,
        RenderLayers::layer(0),
    ));
}

// Crea un círculo relleno
pub fn spawn_circle(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
    pos: Vec2,
    radius: f32,
    color: Color,
) {
    commands.spawn((
        InGameEntity,
        Mesh2d(meshes.add(Circle::new(radius))),
        MeshMaterial2d(materials.add(color)),
        Transform::from_xyz(pos.x, pos.y, MAP_LINES_Z),
        MapLineEntity,
        RenderLayers::layer(0),
    ));
}

// Crea un círculo solo con borde (outline)
pub fn spawn_circle_outline(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<ColorMaterial>>,
    pos: Vec2,
    radius: f32,
    color: Color,
) {
    // Crear anillo usando círculo exterior menos interior
    let outline_thickness = LINE_THICKNESS;

    // Círculo exterior (borde)
    commands.spawn((
        InGameEntity,
        Mesh2d(meshes.add(Circle::new(radius))),
        MeshMaterial2d(materials.add(color)),
        Transform::from_xyz(pos.x, pos.y, MAP_LINES_Z),
        MapLineEntity,
        RenderLayers::layer(0),
    ));

    // Círculo interior (transparente/color del fondo) - simula outline
    commands.spawn((
        InGameEntity,
        Mesh2d(meshes.add(Circle::new(radius - outline_thickness))),
        MeshMaterial2d(materials.add(Color::srgba(0.0, 0.0, 0.0, 0.0))), // Transparente
        Transform::from_xyz(pos.x, pos.y, MAP_LINES_Z + 0.1),            // Ligeramente por encima
        MapLineEntity,
        RenderLayers::layer(0),
    ));
}

// Función auxiliar para aproximar curvas (HaxBall curve format)
pub fn approximate_curve_for_rendering(
    p0: Vec2,
    p1: Vec2,
    curve: f32,
    num_segments: usize,
) -> Vec<Vec2> {
    let mut points = Vec::with_capacity(num_segments + 1);

    let chord = p0.distance(p1);
    let radius = curve.abs();

    // Si el radio es muy pequeño o inválido, retornar línea recta
    if radius < chord / 2.0 {
        points.push(p0);
        points.push(p1);
        return points;
    }

    // Calcular el ángulo subtendido por la cuerda
    let half_angle = (chord / (2.0 * radius)).asin();
    let total_angle = 2.0 * half_angle;

    // Punto medio de la cuerda
    let midpoint = (p0 + p1) * 0.5;

    // Vector de p0 a p1
    let chord_vec = p1 - p0;

    // Vector perpendicular (normalizado)
    let perp = Vec2::new(-chord_vec.y, chord_vec.x).normalize();

    // Distancia del centro a la cuerda
    let height = (radius * radius - (chord / 2.0) * (chord / 2.0)).sqrt();

    // Centro del círculo (curva positiva = perp positivo, negativa = perp negativo)
    let center = if curve > 0.0 {
        midpoint + perp * height
    } else {
        midpoint - perp * height
    };

    // Ángulo inicial (de center a p0)
    let start_angle = (p0.y - center.y).atan2(p0.x - center.x);

    // Determinar dirección de barrido
    let angle_step = if curve > 0.0 {
        -total_angle / num_segments as f32
    } else {
        total_angle / num_segments as f32
    };

    // Generar puntos
    for i in 0..=num_segments {
        let angle = start_angle + angle_step * i as f32;
        let point = Vec2::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        );
        points.push(point);
    }

    points
}

// ============================================================================
// SISTEMA: CÍRCULO DE ZONA DE EXCLUSIÓN PARA JUGADAS A BALÓN PARADO
// ============================================================================

/// Dibuja (o elimina) el círculo de zona de exclusión cuando cambia el estado
/// del partido. La circunferencia tiene el color del equipo que tiene el saque;
/// el radio coincide con el que el host usa para empujar al equipo contrario.
pub fn update_set_piece_visual(
    mut commands: Commands,
    match_game: Res<ClientMatchGame>,
    config: Res<GameConfig>,
    circles: Query<Entity, With<SetPieceCircle>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    if !match_game.is_changed() {
        return;
    }

    // Limpiar círculos anteriores
    for entity in circles.iter() {
        commands.entity(entity).despawn();
    }

    // Obtener set piece activo (solo existe cuando la pelota ya está colocada)
    let Some(ref state) = match_game.0 else { return };
    let Some(ref set_piece) = state.pending_set_piece else { return };
    let Some(position) = set_piece.position() else { return };
    let team = set_piece.team();

    let (r, g, b) = config
        .team_colors
        .get(team as usize)
        .copied()
        .unwrap_or((0.5, 0.5, 0.5));

    let exclusion_radius = config.set_piece_exclusion_radius;
    let ring_thickness = 8.0_f32;
    let inner_radius = exclusion_radius - ring_thickness;

    // Relleno semitransparente
    commands.spawn((
        InGameEntity,
        SetPieceCircle,
        Mesh2d(meshes.add(Circle::new(inner_radius))),
        MeshMaterial2d(materials.add(Color::srgba(r, g, b, 0.12))),
        Transform::from_xyz(position.x, position.y, MAP_LINES_Z + 2.0),
        RenderLayers::layer(0),
    ));

    // Borde sólido (anillo)
    commands.spawn((
        InGameEntity,
        SetPieceCircle,
        Mesh2d(meshes.add(Annulus::new(inner_radius, exclusion_radius))),
        MeshMaterial2d(materials.add(Color::srgba(r, g, b, 0.9))),
        Transform::from_xyz(position.x, position.y, MAP_LINES_Z + 2.1),
        RenderLayers::layer(0),
    ));
}
