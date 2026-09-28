# Pruebas de las fases 0 y 1

## Qué contiene la implementación

- Preparación de pairing y SETUP antes de iniciar la captura.
- Pump activo antes de esperar al buffer inicial del motor; también al añadir un receptor a una captura existente.
- Política estable predeterminada: configuración AirPlay ALAC/NTP y buffers del motor conservados. El slider mantiene 0–3.000 ms, confirmación y 10 segundos entre cambios.
- Opción «Búfer local reducido (experimental)», desactivada por defecto y modificable con la reproducción parada.
- En el modo experimental: cola de captura limitada a 60 ms, cola de entrada de siete bloques de 352 frames, buffer del motor de hasta 80 ms, objetivo inicial de 24 ms y cola de envío de dos paquetes. El motor descarta audio con más de 120 ms desde su timestamp de captura al preparar/enviar. Los tamaños efectivos se redondean a paquetes completos.
- Métricas de las últimas seis sesiones, con tiempos de las etapas, contadores de saturación/descartes, edad del audio, buffer del encoder, paquetes enviados, errores UDP y resultados de heartbeats.
- Exportación voluntaria desde «Diagnóstico de audio». El JSON usa números de receptor y campos definidos; no contiene nombres, IP, credenciales ni logs en bruto.
- Pruebas de saturación, caducidad, pre-roll, cierre de hilos y continuidad de secuencias/cifrado. CI de Windows con instalador de prueba como artefacto, sin publicar una release.

Los límites locales anteriores **no son una promesa de 120 ms de retraso audible**. WASAPI, la red y el HomePod pueden añadir demora. Los timestamps son de entrega de software; en Windows se estima la antigüedad de los chunks agrupados por el audio pendiente, sin presentar esa estimación como un timestamp de hardware.

## Comparación con un HomePod

1. Guardar la referencia con la versión 0.1.7: mismo PC, HomePod, red, fuente de audio, volumen y valor del slider. Anotar cuánto tarda en empezar y si hay cortes.
2. Instalar el build de prueba de la PR. Dejar «Búfer local reducido» desactivado y repetir inicio, música continua, silencio/reanudación, parada y vuelta a reproducir.
3. Descargar un diagnóstico tras al menos 20 segundos de reproducción y otro después de parar. Los contadores de captura se actualizan cada 10 segundos y al terminar el pump; las métricas del motor se leen al exportar.
4. Parar, activar «Búfer local reducido» y repetir con los mismos valores. Guardar otro diagnóstico.
5. Probar 0, 100, 300, 1.000 y 3.000 ms, respetando la confirmación y el intervalo de 10 segundos. Los valores bajos pueden producir cortes o ser rechazados por el receptor.
6. Para cuantificar demora audible, grabar una señal de referencia y la salida del HomePod con una referencia temporal independiente. La sincronización visual de un vídeo y los contadores de paquetes no bastan para afirmar una cifra acústica exacta.

Registrar versión de Windows/HomePod, tipo de conexión, retardo percibido y cortes por prueba junto al JSON. Esos datos de contexto se añaden voluntariamente; la exportación no los obtiene de dispositivos ni cuentas.

## Casos de regresión

- Vídeo/música de una aplicación que cambie entre sonido y silencio varias veces.
- Fuente de Windows a 48 kHz: velocidad y canales correctos tras la conversión a 44,1 kHz.
- Carga de CPU durante la reproducción: en modo experimental, comprobar que el retraso no queda acumulado después del parón.
- Cambiar de receptor a uno que falla: el receptor anterior continúa disponible.
- Parar y cerrar la aplicación durante reproducción. Revisar que no continúa enviando audio.
- Desconectar la fuente capturada: se informa del fallo; una captura que muere durante el arranque no se presenta como reproducción activa.
- Volumen y confirmación de latencia continúan funcionando. El ajuste experimental solo cambia los buffers locales.

El cambio entre dos HomePods, el Denon y la reproducción estéreo conservan sus pruebas pendientes. Estas fases no justifican cerrar #13, #20, #2 o #14.

## Interpretación del diagnóstico

El build de CI conserva el número 0.1.7 durante esta prueba, pero incluye
`build_revision` en el JSON para distinguir su commit de la release. En builds
locales sin `AIRSEND_BUILD_REVISION`, ese campo queda vacío.

| Campo | Interpretación |
| --- | --- |
| `requested_receiver_latency_ms` | Valor solicitado con el slider; no medición acústica. |
| `timings.start_ms` | Duración del arranque del motor y pre-roll; permite detectar esperas agotadas. |
| `capture.queued_ms` | Audio pendiente en la cola de captura. |
| `capture.dropped_oldest` / `stale_drops` | Audio antiguo eliminado por saturación/caducidad. |
| `input_queue_drops` | Bloques descartados en la entrada del motor; el modo estable descarta el nuevo y el experimental el antiguo. |
| `encoder_buffer_ms` | Ocupación observada del buffer del motor. |
| `encoder_underruns` | Veces que el motor se quedó sin audio para enviar. |
| `capture_delivery_to_udp_p95_ms` | Percentil 95 de las últimas 256 muestras de edad del audio realmente enviado por UDP; excluye el retardo del receptor. |
| `feedback_failures` / `send_errors` | Fallos de control RTSP o de envío local, separados de los underruns de captura. |
| `last_failure_stage` | Etapa del último fallo, sin el contenido bruto del error. |

Antes de adoptar el modo experimental como predeterminado, debe demostrar mejora en la comparación y estabilidad en hardware real. Mantener el modo estable permite volver a la política anterior durante las pruebas.
