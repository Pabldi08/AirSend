# AirSend: análisis de AirFlash y plan de mejoras

Análisis original: 28 de septiembre de 2026. Actualización: 29 de septiembre de 2026. La integración de las fases 0–6 y de la corrección de CPU está preparada en `codex/airflash-improvements`. Las funciones nuevas de búfer reducido, silencio local y grupos siguen optativas. La cobertura implementada y las comprobaciones pendientes están en [integration-testing.md](integration-testing.md); la comparación de latencia conserva [phase-01-testing.md](phase-01-testing.md).

La tabla y los hallazgos siguientes describen la referencia de 0.1.7 anterior a esta integración. Implementación y aceptación en hardware son estados distintos: todavía no se ha certificado Denon, silencio Windows ni parejas HomePod reales.

## Alcance y conclusión

Se ha comparado el código de AirSend 0.1.7, en `main` (`b0836a828eafbcaf30db43c8f492dff4a1ee0d5f`), con [AirFlash](https://github.com/Ding-Kyoma/AirFlash/tree/c046a7d43f5be4306ded1e38cd906c9b6438c4a5). También se ha inspeccionado la dependencia `Pabldi08/airplay2-rs` fijada por AirSend en `655b2768e2a17aa64f84d0b5b36325018963e909`, y los comentarios de las issues abiertas #2, #12, #13, #14 y #20.

AirFlash aporta ideas útiles para controlar la antigüedad del audio, gestionar sesiones y representar parejas estéreo. AirSend puede incorporarlas manteniendo Rust y Tauri, con cambios pequeños y comprobaciones sobre la reproducción que ya funciona.

Este análisis es de código y documentación. No se ha ejecutado AirFlash ni se han medido sus prestaciones en un HomePod. Su perfil de 120 ms es un objetivo solicitado al receptor, no una medición del retraso entre el sonido generado y el sonido reproducido. Su motor limita las sesiones a un receptor o una pareja de dos; esto tampoco demuestra soporte general de reproducción en varias habitaciones. [Documentación de AirFlash](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/README.md), [límites del motor](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/native/airflash-engine/README.md), [validación de sesiones](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/native/airflash-engine/src/session.rs#L85).

## Comparación relevante

| Área | AirSend actual | Idea aprovechable de AirFlash | Prioridad |
| --- | --- | --- | --- |
| Latencia local | Colas por número de bloques; al llenarse la captura descarta audio nuevo. El slider configura la negociación con el receptor. | Marcas de tiempo, colas limitadas por duración y eliminación de audio atrasado. | Muy alta |
| Inicio de reproducción | La captura comienza antes de conectar; el productor del decodificador empieza después de esperar a que el decodificador arranque. | Preparar la sesión y coordinar la disponibilidad de captura y envío. | Muy alta |
| Identidad y capacidades | Descubre modelo, identidad y features, pero vuelve a construir el receptor sin esos datos al conectar. | Identidad estable, metadatos AirPlay/RAOP combinados y consulta de capacidades. | Alta |
| Audio de Windows | Captura la salida predeterminada; no ofrece selección ni silencio local. | Selector de salida de Windows, notificaciones de cambios y restauración del mute. | Alta; #12 |
| Estado de sesión | Heartbeats con errores registrados; sesiones independientes y estados de conexión y reproducción separados. | Sesiones cancelables, identificación de eventos antiguos y fallos comunicados al controlador. | Alta; #13 y #20 |
| Volumen | Volumen global enviado a los receptores. | Lectura del volumen real, confirmación de escrituras y ajustes por receptor. | Media |
| Distribución | Instalador NSIS y release en borrador con notas automáticas. | Comprobación manual de actualizaciones, hashes y comprobaciones de instalación. | Media |
| Dos HomePods | Una captura compartida alimenta sesiones independientes. No hay coordinación de grupo integrada en AirSend. | Pareja descubierta como una unidad, reloj compartido y coordinación PTP. | Última fase; #2 y #14 |

Fuentes de implementación de AirFlash: [captura y colas](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/native/airflash-engine/src/live.rs), [descubrimiento y parejas](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/desktop/AirFlash.Core/Receiver.cs), [controlador de sesiones](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/desktop/AirFlash.Core/SessionController.cs), [audio de Windows](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/desktop/AirFlash.App/Services/AudioService.cs), [volumen](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/desktop/AirFlash.Core/DeviceVolumeState.cs).

## Hallazgos concretos en AirSend

### 1. El valor 0 ms no elimina los buffers internos

En `crates/audio-capture/src/windows.rs`, la cola admite 64 bloques de 352 frames a 44,1 kHz: aproximadamente 511 ms de audio si se llena. `try_send` descarta el bloque entrante cuando está llena y conserva los anteriores. En `src-tauri/src/lib.rs`, `prepare_stream` inicia esa captura antes de abrir el receptor y crea el pump después de abrirlo.

Además, la dependencia fija un buffer de **capacidad de 2.000 ms**, espera un llenado inicial del 50 % y establece un máximo de espera de 5 segundos. AirSend obtiene el sender después de esperar a `start_streaming_live`, por lo que todavía no puede alimentar esa espera inicial. La lectura del código predice una espera de arranque innecesaria y conservación de audio antiguo; hay que confirmar ambos efectos con logs y mediciones. Los umbrales de ese buffer son independientes del slider. No se deben sumar las capacidades de las colas y presentarlas como una medición de latencia real. [Buffer del fork](https://github.com/Pabldi08/airplay2-rs/blob/655b2768e2a17aa64f84d0b5b36325018963e909/crates/airplay-audio/src/streamer.rs#L497), [espera inicial](https://github.com/Pabldi08/airplay2-rs/blob/655b2768e2a17aa64f84d0b5b36325018963e909/crates/airplay-audio/src/streamer.rs#L730).

La mejora debe actuar sobre el ciclo de vida y los buffers del fork, además de las colas propias. Reducir solamente el valor enviado al HomePod no basta.

### 2. Se pierde información del receptor al conectar

`open_output` y `connect_device` construyen `DeviceDescriptor` con `mac`, `model` y `features` vacíos. El constructor termina asignando modelo y features de HomePod a receptores desconocidos. La detección de AirPlay 2 usa heurísticas de versión/formato, y no conserva capacidades suficientes para decidir autenticación, códec o agrupación.

Hay que corregir también `add_manual_device`: actualmente guarda la cabecera RTSP `Server` dentro de `features`. Antes de propagar los metadatos al protocolo, ese dato debe tener un campo separado; de lo contrario una cadena de servidor se trataría como un mapa de capacidades.

### 3. El caso Denon contiene problemas diferentes

Los comentarios de #13 indican que un filtro de AdGuard bloqueaba la conexión, que cambiarlo permitió despertar el Denon, y que existía además un fallo de captura. También aportan los TXT reales del AVC-X4800H y una limitación con contraseña HEOS. Esto justifica pruebas dirigidas de red, captura y autenticación. No demuestra que cambiar a PCM o PTP resuelva el caso. [Comentario sobre el filtro](https://github.com/Pabldi08/AirSend/issues/13#issuecomment-5845218663), [metadatos aportados](https://github.com/Pabldi08/AirSend/issues/13#issuecomment-5845373342).

### 4. Silenciar la salida requiere una prueba por dispositivo

AirFlash guarda el mute anterior de la salida y lo restaura al parar. Es un buen patrón de gestión, pero #12 informa de que silenciar Windows también silencia el HomePod. WASAPI puede capturar desde un pin del hardware o desde la mezcla del motor; Windows puede aplicar el mute en hardware o software. Por tanto, no está probado que el mismo procedimiento funcione en todos los equipos. [Loopback de Microsoft](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording), [control de volumen del endpoint](https://learn.microsoft.com/en-us/windows/win32/api/endpointvolume/nn-endpointvolume-iaudioendpointvolume).

### 5. La sincronización necesita una sesión de grupo

Enviar las mismas muestras a dos sesiones independientes no asegura que se reproduzcan a la vez. AirFlash comparte reloj y cronología RTP, y conserva identidad, líder y miembros de una pareja. El fork de AirSend ya contiene `setup_for_group`, `send_setpeers`, primitivas PTP y un ejemplo de grupo. Conviene evaluarlas antes de añadir un segundo motor. Ese ejemplo documenta una dificultad con el modo de emisor PTP maestro, por lo que cambiar una opción de NTP a PTP no constituye una solución completa. [Ejemplo del fork](https://github.com/Pabldi08/airplay2-rs/blob/655b2768e2a17aa64f84d0b5b36325018963e909/crates/airplay-client/examples/test_group.rs#L249).

## Plan de implementación, en orden

Cada fase tiene un resultado comprobable. El plan original proponía validar cada fase antes de la siguiente; por petición del usuario se integra el conjunto en una misma rama, con la aceptación en dispositivos todavía pendiente. Complejidad relativa: baja, media o alta; los plazos dependen especialmente de disponer de testers y hardware.

### Fase 0 — Medir y fijar la referencia

**Complejidad: media. Dependencias: ninguna.**

- Registrar tiempo de pairing/setup/arranque, edad y ocupación de cada cola, frames descartados, underruns, paquetes enviados y fallos de heartbeat por receptor.
- Añadir marcas de tiempo monotónicas a los bloques capturados y mantener su trazabilidad hasta el envío.
- Preparar exportación voluntaria de diagnóstico con versión, configuración y etapa del fallo; ocultar credenciales y datos identificativos en la exportación.
- Reproducir la espera inicial con una fuente simulada y medir 0.1.7 en Windows con el HomePod actual.

**Archivos:** captura, `streaming.rs`, pump y contadores de `src-tauri/src/lib.rs`; métricas del fork.

**Criterio de aceptación:** distinguir retraso local, latencia solicitada y retraso acústico medido. Obtener una referencia repetible de inicio, silencio/reanudación y reproducción sostenida, sin cambiar el comportamiento del protocolo.

### Fase 1 — Corregir arranque y audio atrasado

**Complejidad: alta. Dependencias: fase 0.**

- Separar creación del canal, arranque del productor y espera de preparación; proporcionar muestras mientras el consumidor espera, con cancelación y cierre si falla alguna etapa.
- Iniciar captura cerca del momento de envío y eliminar muestras previas a la sesión. Durante un cambio de receptor, la nueva captura debe avanzar aunque todavía no se transmita.
- Hacer configurables en el fork los buffers y umbrales de la ruta de audio en directo, manteniendo separados el presupuesto local y la latencia negociada con el receptor.
- Limitar las colas por tiempo y descartar audio antiguo cuando haya saturación o un parón del sistema. Mantener coherentes los timestamps RTP y los contadores de cifrado al saltar audio; nunca reiniciarlos de forma que reutilicen un nonce.
- Conservar el rango 0–3.000 ms, la confirmación y el intervalo de 10 segundos. Si se añaden presets, expresarlos como preferencias de latencia/estabilidad, sin prometer una cifra acústica.
- Activar la nueva política primero como modo experimental. Mantener la política estable disponible hasta demostrar que la alternativa no aumenta los cortes.

**Archivos:** captura, `streaming.rs`, `prepare_stream` y `airplay-audio` del fork. Fijar la nueva revisión manteniendo los parches de Windows QoS/MMCSS documentados en `CONTRIBUTING.md`.

**Criterios de aceptación:** la fuente simulada demuestra que no se agota la espera inicial por falta de productor; tras un parón se recupera audio reciente; los límites de edad se cumplen en pruebas de carga. En Windows se mide una reducción frente a 0.1.7 sin regresiones de silencio/reanudación o estabilidad. 0 ms sigue significando una solicitud mínima al receptor, sin promesa de cero retraso físico.

### Fase 2 — Descubrimiento y compatibilidad basados en datos reales

**Complejidad: media. Dependencias: fase 0; aplicar por separado de la fase 1.**

- Definir en Rust un receptor con identidad estable y rutas AirPlay/RAOP; pasar su ID al comando de conexión y resolver los detalles en el backend.
- Conservar TXT relevantes: `deviceid`, `model`/`am`, `features`/`ft`, `cn`, `et`, versiones, indicadores de acceso y, cuando existan, `tsid`, `gpn` e `igl`. Separar los datos de sondeo del mapa de features.
- Contrastar capacidades con `/info` cuando esté disponible y tratar las desconocidas como desconocidas. Mantener la ruta actual para el HomePod validado durante la transición.
- Procesar eliminación/caducidad de servicios y cambios de red, para que un receptor desconectado no permanezca disponible indefinidamente. Conservar entradas manuales como tales, indicando su disponibilidad.
- Usar los TXT del Denon como fixture y diagnósticos por etapa; probar el caso real con el usuario de #13. Añadir PIN o credenciales persistentes únicamente para los modos de autenticación que requiera y permita el receptor.

**Archivos:** `discovery.rs`, `pairing.rs`, `probe.rs`, comandos y caché del backend, `ui/src/devices.ts`.

**Criterios de aceptación:** deduplicación estable AirPlay/RAOP, parser de los TXT del Denon, conexión manual válida y conservación de la conexión del HomePod actual. #13 se cierra únicamente cuando su autor confirme reproducción y continuidad; el Mac se considera una validación adicional por modelo y configuración, no compatibilidad universal.

### Fase 3 — Elegir la salida capturada y resolver #12

**Complejidad: media para el selector; alta si el mute corta la captura. Dependencias: fase 0.**

- Añadir «Fuente de audio de Windows»: salida predeterminada o salida concreta, persistida por ID. Aclarar que elegirla captura las aplicaciones que ya reproducen en ella; no redirige automáticamente todas las aplicaciones.
- Detectar cambios de salida y desconexiones mediante notificaciones de Windows. Seguir los cambios solo en modo predeterminado; si desaparece una salida fija, mostrar el fallo y permitir elegir otra.
- Prototipar «Silenciar la salida local mientras se transmite», desactivado inicialmente. Aplicarlo tras confirmar reproducción y comprobar, durante audio conocido, que la captura conserva señal y el HomePod continúa sonando.
- Guardar el estado anterior por salida, restaurarlo al parar/fallar/cerrar y registrar cambios hechos por AirSend para respetar acciones posteriores del usuario. Prever recuperación al siguiente inicio después de un cierre abrupto.
- Si el mute elimina la señal capturada en un driver, restaurar el estado y declarar la limitación. Evaluar por separado captura de proceso o una salida virtual configurada por el usuario; el selector por sí solo no resuelve ese caso.

**Archivos:** `audio-capture` de Windows, ajustes y comandos Tauri, interfaz e i18n.

**Criterios de aceptación:** solo suena el HomePod en un equipo compatible; sigue llegando audio después de silenciar; se restaura el estado anterior en todas las salidas de la sesión. Probar auriculares, HDMI/monitor y USB disponibles, además de cambios predeterminados y retirada de dispositivo. #12 permanece abierta si solo se ha añadido el selector y todavía no se ha resuelto el silencio local del caso reportado.

### Fase 4 — Estado fiable, recuperación y cambios de receptor

**Complejidad: alta. Dependencias: fases 0 y 2.**

- Unificar el ciclo de sesión: desconectado, preparando, transmitiendo, recuperando y error. La UI debe reflejar el receptor cuya sesión está activa.
- Asociar eventos y operaciones a un ID/generación; ignorar callbacks antiguos y cancelar preparaciones si se solicita parar o cambiar.
- Comunicar al controlador fallos de captura, transporte y heartbeats repetidos. Diferenciar cola llena de canal cerrado.
- Añadir reconexión opcional, con pocos intentos, esperas crecientes y cancelación inmediata. No reintentar indefinidamente errores de autenticación o configuración.
- Mantener el cambio preparado antes de retirar el receptor anterior y la recuperación del anterior si falla el nuevo. Restaurar el mute cuando se pierda definitivamente la sesión.

**Archivos:** control de estado en `src-tauri/src/lib.rs`, heartbeat y errores de `streaming.rs`, eventos de UI.

**Criterios de aceptación:** pruebas de stop durante pairing, destino fallido, evento antiguo, pérdida de Wi-Fi y cierre de captura. Comprobación inicial con HomePod + Mac si el Mac acepta la sesión. #20 sigue abierta hasta validar el cambio entre dos HomePods reales.

### Fase 5 — Ajustes por receptor y releases más fáciles de comprobar

**Complejidad: media. Dependencias: fase 2 para identidad y fase 4 para estado.**

- Guardar volumen, latencia y preferencias de reconexión por identidad del receptor, migrando el ajuste global existente como valor inicial.
- Leer volumen del receptor cuando lo soporte; reflejar cambios físicos y confirmar escrituras sin permitir que una lectura antigua sobrescriba una petición nueva. Degradar de forma explícita cuando la lectura no esté disponible.
- Añadir «Buscar actualizaciones» manual y abrir la release oficial; conservar la instalación manual existente.
- Ejecutar en Windows las pruebas de regresión pertinentes antes de producir el instalador, publicar SHA256 y comprobar actualización desde 0.1.7 conservando ajustes.
- Corregir el roadmap: las notas automáticas de release ya existen. Mantener el identificador del producto y el flujo de release en borrador.

**Criterios de aceptación:** cambios físicos de volumen reflejados o limitación indicada, preferencias separadas, migración sin pérdida de ajustes, hashes correspondientes al `.exe` e instalación/actualización correcta.

### Fase 6 — Pareja estéreo y reproducción sincronizada, al final

**Complejidad: alta. Dependencias: fases 1, 2 y 4; validación final con dos HomePods.**

- Representar una pareja configurada en Apple Home como unidad lógica según metadatos reales. Mostrar miembros y disponibilidad; una pareja incompleta no debe presentarse como estéreo validado.
- Mantener separados los conceptos de pareja estéreo y dos receptores independientes reproduciendo el mismo contenido.
- Evaluar primero las primitivas de grupo del fork: descubrimiento de líder, `SETPEERS`, negociación de timing y un reloj/cronología RTP compartidos. Verificar las exigencias de PTP y de red en Windows.
- Usar captura común y coordinación de envío, preservando claves y contadores de cifrado propios de cada sesión. Limitar retransmisiones de audio vencido para que no retrasen audio nuevo.
- Probar canales izquierdo/derecho, inicio coordinado, deriva, pérdida de un miembro y vuelta del miembro. Definir explícitamente si se pausa la pareja o continúa el receptor sano.
- Integrarlo con el ajuste de reproducción múltiple y las confirmaciones existentes. Publicarlo inicialmente como experimental.

**Criterios de aceptación:** simulaciones de relojes, secuencias y retrasos de red; después reproducción y cambios reales con dos HomePods, tanto independientes como pareja configurada. Una prueba HomePod + Mac aporta cobertura preliminar, pero no certifica sincronización o canales de una pareja HomePod. #2 y #14 requieren esa validación; #20 requiere su propia prueba de cambio. El usuario de #14 ya se ofreció a probar. [Oferta de pruebas](https://github.com/Pabldi08/AirSend/issues/14#issuecomment-5768387478).

## Comprobaciones que protegen la funcionalidad actual

| Escenario | Comprobación necesaria |
| --- | --- |
| Un HomePod con software 27.0 | Inicio, reproducción, volumen, pausa/silencio, reanudación y parada, comparados con 0.1.7. |
| Latencia solicitada 0, 100, 300, 1.000 y 3.000 ms | Tiempo de inicio, edad del audio al enviar, underruns y demora audible. Usar una referencia y grabación independientes para cuantificar la demora acústica. |
| Carga o parón de envío | El audio antiguo se descarta con límites; no queda un retraso acumulado permanente ni se reutilizan contadores de cifrado. |
| Fuente Windows a 48 kHz | La conversión actual a 44,1 kHz funciona sin cambiar velocidad, canales o duración. |
| Red caída o aplicación cerrada | Estado visible coherente, recursos liberados, reintentos cancelables y mute restaurado. |
| Denon AVC-X4800H | Confirmación de sonido y continuidad por su usuario, separando filtro de red, captura y autenticación. |
| Dos HomePods | Cambio real, canales de pareja y deriva durante reproducción sostenida. Las simulaciones no sustituyen esta prueba. |

No hace falta añadir un resampler nuevo solo para convertir 48 a 44,1 kHz: AirSend ya usa `AUTOCONVERTPCM`. La corrección adaptativa de deriva de AirFlash merece evaluarse si las métricas muestran divergencia entre los relojes de captura y envío; entonces se hará un prototipo específico con pruebas de continuidad y calidad. Tampoco se propone migrar a WPF ni reemplazar `mdns-sd` solo por usar una API distinta.

## Organización del trabajo y cierre de issues

- Trabajar con `main` y una única rama activa `codex/airflash-improvements`. Reunir la implementación del plan completo en la PR #21, por petición del usuario. Conservar las comprobaciones de cada fase y la validación de hardware antes de promocionar las funciones experimentales.
- Los cambios necesarios en `airplay2-rs` se revisan en su repositorio y se fijan por commit en AirSend. Los PRs del fork y de AirSend deben explicar su dependencia.
- Mantener las funciones experimentales optativas y la ruta de un HomePod conocida durante las pruebas. Preparar builds de prueba antes de promocionarlas a una release estable.
- Mantener #13 abierta hasta la confirmación del usuario Denon; #20 hasta probar dos HomePods; #2 y #14 hasta comprobar reproducción múltiple y estéreo según su alcance. Ninguna se cierra por la mera presencia de código.
- Implementar las ideas en el código existente. Si se reutiliza código concreto de AirFlash, conservar atribución y los avisos de su licencia GPLv3+; AirSend declara `GPL-3.0-or-later`. [Licencia de AirFlash](https://github.com/Ding-Kyoma/AirFlash/blob/c046a7d43f5be4306ded1e38cd906c9b6438c4a5/LICENSE).

**Siguiente comprobación:** instalar el artefacto Windows de la integración y seguir `integration-testing.md`. La corrección NTP está incorporada mediante la PR #3 del fork y cubierta por regresiones. La mejora de CPU en la máquina del usuario y los escenarios de hardware no se dan por certificados por la presencia de código.
