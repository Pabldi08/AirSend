# Revisión de issues — 26 de octubre de 2026

Se consultaron todas las issues y PR abiertas de AirSend. Había 9 issues y ninguna PR abierta al empezar. La rama local anterior estaba limpia; la base usada es `origin/main` (`e93535b`, PR #21 integrada). La versión publicada sigue siendo 0.1.7.

| Issue | Estado observado y siguiente validación |
| --- | --- |
| [#2](https://github.com/Pabldi08/AirSend/issues/2) | Hay reproducción independiente y grupo experimental de dos receptores. Validar con dos receptores físicos; no cerrar por pruebas unitarias. |
| [#12](https://github.com/Pabldi08/AirSend/issues/12) | Hay selección de salida y mute reversible con recuperación. Falta comprobar drivers Windows que preserven loopback al silenciar, cambios de salida y recuperación tras crash. |
| [#13](https://github.com/Pabldi08/AirSend/issues/13) | Se conservan capacidades, autenticación y rutas del Denon; las pruebas cubren la metadata. Falta reproducir y autenticar en el AVR X4800H real. |
| [#14](https://github.com/Pabldi08/AirSend/issues/14) | Hay parejas configuradas y grupo experimental. Faltan canales L/R, deriva de 30 minutos y pérdida de un miembro en hardware. |
| [#20](https://github.com/Pabldi08/AirSend/issues/20) | El cambio de receptor reemplaza la sesión y preserva la anterior si falla. Falta comprobar audio y UI al alternar HomePods. |
| [#22](https://github.com/Pabldi08/AirSend/issues/22) | La dependencia fijada ya incluye cierre de timing y las sesiones desconectan al parar. Faltan perfiles CPU/hilos tras diez paradas y diez cambios en Windows. |
| [#23](https://github.com/Pabldi08/AirSend/issues/23) | El código acepta y envía `(0, 0)` como búfer, pese al informe de silencio. Se aborda por separado en `issue/buffer-minimo`: evitar 0 ms y migrar ajustes antiguos a 100 ms, el mínimo confirmado por el informante. Esto no certifica latencia acústica de 100 ms. |
| [#24](https://github.com/Pabldi08/AirSend/issues/24) | Rama `issue/homepod-os27`: integrar parser de UUID compuestos de airplay2-rs#5 con autoría original, elegir PTP anunciado, corregir offset BMCA y recuperación RTP, mantener errores visibles y recuperar ritmo de silencio con temporizadores Windows gruesos. Falta validar en HomePod OS 27. |
| [#25](https://github.com/Pabldi08/AirSend/issues/25) | Sin descripción ni pasos y con título promocional. Eliminada por petición expresa del propietario. |

La PR abierta [airplay2-rs#5](https://github.com/Pabldi08/airplay2-rs/pull/5) se revisó como dependencia de #24. Acepta los dos formatos observados de `gid`/`pgid` (`UUID+UUID` y `UUID+1+UUID`), conserva `tsid` simple y evita descartar el receptor por IDs de grupo ilegibles. No se ha fusionado ni cerrado automáticamente.

Las pruebas locales comprueban la lógica y los paquetes; no sustituyen las verificaciones de [integration-testing.md](integration-testing.md). No se publica una release ni se cierran issues que necesitan hardware.
