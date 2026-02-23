# ADR-002: Estrategia de Migración a WebAssembly

**Fecha:** 2026-02-21
**Estado:** Diferido
**Autores:** Anibal

## Contexto

Se evaluó la viabilidad de compilar el cliente de kinetic_ball (`kinetic_ball` binary) a
WebAssembly para permitir ejecutarlo directamente en el browser sin instalación.

El proyecto tiene dos binarios con restricciones muy distintas:

- **`kinetic_ball`** (cliente + host): candidato a WASM para la parte cliente.
- **`kinetic_ball_server`** (proxy REST+WS): servidor nativo, nunca irá a WASM.

El host (`host.rs`) tampoco es candidato a WASM — es un servidor autoritario de física que
corre en un proceso separado.

## Análisis de Compatibilidad

### Lo que funciona en WASM sin cambios

| Componente | Estado |
|---|---|
| Bevy 0.17 | ✅ Soportado |
| bevy_rapier2d | ✅ Soportado (Rapier es WASM-compatible) |
| bevy_egui | ✅ Soportado |
| matchbox_socket | ✅ Diseñado para WASM — usa WebRTC API del browser internamente |
| tokio (`rt`, `sync`, `time`, `macros`) | ✅ Soportado con features limitadas |
| `tokio::spawn` | ✅ Funciona en WASM |
| `tokio::time::sleep` | ✅ Funciona en WASM (puede panic si la plataforma no tiene timers) |

### Lo que requiere cambios

| Componente | Problema | Solución |
|---|---|---|
| `std::thread::spawn` | No existe en `wasm32-unknown-unknown` | `wasm_bindgen_futures::spawn_local` con `#[cfg]` |
| `tokio::runtime::Builder` + `block_on` | Inútil en WASM (no hay hilo donde bloquear) | Reemplazar por `spawn_local` en WASM |
| `gilrs` | No soporta WASM | Compilación condicional; usar Bevy Gamepad API en WASM |
| `std::fs` / `dirs` (config) | No hay filesystem en browser | `#[cfg(wasm32)]` → `localStorage` o usar defaults siempre |
| `rustls::crypto::ring::default_provider()` | Browser maneja TLS directamente | Envolver en `#[cfg(not(target_arch = "wasm32"))]` |

### Tokio en WASM — aclaraciones

Tokio soporta WASM para los features: `sync`, `macros`, `io-util`, `rt`, `time`.
El feature `full` y `net` no compilan en WASM. No se puede usar
`tokio::runtime::Builder::new_multi_thread()`.

El patrón actual del cliente es:

```rust
// cliente actual (nativo)
std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all().build().unwrap();
    rt.block_on(async {
        start_webrtc_client(...).await;  // usa tokio::spawn y tokio::time::sleep
    });
});
```

El hilo solo existe para poder llamar `block_on`. En WASM esto se reemplaza por:

```rust
#[cfg(target_arch = "wasm32")]
wasm_bindgen_futures::spawn_local(async move {
    start_webrtc_client(...).await;  // el interior no cambia
});
```

Todo el interior de `start_webrtc_client` (`tokio::spawn(loop_fut)`,
`tokio::time::sleep`) funciona igual en WASM.

### WASM Threads (SharedArrayBuffer)

Existe soporte experimental para threads reales en WASM mediante:
- `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"`
- Headers HTTP: `Cross-Origin-Opener-Policy: same-origin` y `Cross-Origin-Embedder-Policy: require-corp`

Esto permitiría mantener `std::thread::spawn` vía crates como `wasm_thread`.
Sin embargo, **no es necesario** para este proyecto: el hilo de red del cliente no
requiere paralelismo real, solo un contexto async. `spawn_local` es la solución correcta.

## Decisión

**Se difiere la migración a WASM.**

El alcance del trabajo necesario es acotado y bien entendido, pero no es prioridad en este
momento. La arquitectura actual no presenta obstáculos fundamentales para una futura migración.

## Trabajo necesario cuando se retome

1. **`client.rs`**: reemplazar `std::thread::spawn` + `rt.block_on` por
   `wasm_bindgen_futures::spawn_local` bajo `#[cfg(target_arch = "wasm32")]`.
   El interior de `start_webrtc_client` no requiere cambios.

2. **`gilrs`**: envolver toda la inicialización y uso en
   `#[cfg(not(target_arch = "wasm32"))]`. En WASM, los gamepads se leen vía
   `read_bevy_gamepad_input` (Bevy Gamepad API), que ya existe y es WASM-compatible.

3. **`keybindings.rs`** (carga de config): envolver `std::fs` y `dirs` en
   `#[cfg(not(target_arch = "wasm32"))]`. En WASM usar defaults o `localStorage`.

4. **`main.rs`**: envolver `rustls::crypto::ring::default_provider()` en
   `#[cfg(not(target_arch = "wasm32"))]`.

5. **`Cargo.toml`**: ajustar features de tokio para excluir `net` en WASM, agregar
   `wasm-bindgen-futures` como dependencia condicional.

6. **Build tooling**: agregar `wasm-pack` o `cargo build --target wasm32-unknown-unknown`,
   configurar servidor con headers COOP/COEP si se usan features que requieren
   SharedArrayBuffer.

## Consecuencias

- El host (`host.rs`) y el proxy (`kinetic_ball_server`) permanecen como binarios nativos.
- El cliente WASM se conectaría al mismo proxy WebRTC existente sin cambios en el servidor.
- `matchbox_socket` ya abstrae la diferencia entre native y WASM — la capa de red no
  requiere cambios lógicos.
