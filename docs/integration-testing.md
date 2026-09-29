# Pruebas de la integración de AirFlash y CPU

La rama `codex/airflash-improvements` reúne las fases 0–6 del plan. Las funciones nuevas de búfer reducido, silencio local, parejas y envío sincronizado son optativas. La release publicada 0.1.7 no contiene esta integración. El instalador de CI conserva ese número, pero el diagnóstico contiene `build_revision` y el artefacto incluye `SHA256SUMS`.

## Cobertura implementada

| Fase | Código integrado | Validación pendiente en dispositivos |
| --- | --- | --- |
| CPU | Cierre de NTP/PTP, control y captura; EOF RTSP sin bucle; peticiones interrumpidas requieren sesión nueva; parada usa desconexión permanente. MMCSS queda limitado a los hilos de audio, sin elevar todo el proceso. | CPU tras muchos cambios/paradas y continuidad bajo carga en Windows. |
| 0–1 | Diagnóstico acotado, captura después de SETUP, alimentación antes del pre-roll y caducidad local optativa. | Comparación de demora/cortes de `phase-01-testing.md`. |
| 2 | Identidad estable, TXT AirPlay/RAOP conservados y combinados, cabecera `Server` separada, retiradas mDNS, `/info` real aplicado antes de pairing y sin capacidades HomePod inventadas. Se conserva la última metadata tras una retirada para recuperar la sesión; se marca no disponible. | Sonido en Denon AVC-X4800H y Mac; contraseñas/autenticación HEOS siguen sin certificarse. |
| 3 | Enumeración/selección de salidas activas, seguimiento del dispositivo predeterminado, parada al retirar una salida seleccionada, mute optativo con callback, restauración y fichero de recuperación tras crash. | Drivers concretos: captura con la salida silenciada, cambios de predeterminado y recuperación al reiniciar. |
| 4 | Cancelación de pairing/preparación, generaciones de sesión, destino anterior conservado si falla el cambio; heartbeat/UDP/captura controlados; hasta tres recuperaciones opcionales tras 1, 2 y 4 s. | Caída de Wi-Fi, cambio entre dos HomePods y ausencia de recursos retenidos. |
| 5 | Volumen/latencia/reconexión por identidad, migración desde ajustes globales, lectura de `initialVolume` de `/info` cuando existe, escritura confirmada, lecturas antiguas filtradas, acceso manual a releases y hashes. | Botones físicos del receptor y actualización desde 0.1.7 con preferencias conservadas. |
| 6 | Pareja lógica por `tsid`, líder y miembros, rechazo de parejas incompletas, grupo PTP de exactamente dos receptores con captura/encoder/cronología/envío común y cifrado/caché por miembro. Retransmisión de audio vencido limitada a 200 ms o al límite experimental. | Canales L/R, inicio audible coordinado, deriva prolongada y recuperación de miembros reales. |

La reproducción múltiple anterior conserva sesiones independientes. El botón de grupo experimental crea la ruta coordinada de dos receptores. Quitar o perder un miembro detiene todo el grupo; si se activa reconexión se recupera el grupo completo. No se admite añadir un tercer receptor a esa ruta. La identificación `tsid` evita confundir el grupo de una habitación (`gid`) con una pareja estéreo.

En los grupos, `transport_scope: "group"` identifica contadores de envío/encoder compartidos; los heartbeats y tiempos de conexión son por miembro. La ruta independiente mantiene contadores por receptor.

Las pruebas UDP verifican cabeceras RTP idénticas, paquetes cifrados distintos, secuencias/nonces y cierre; no certifican sincronización audible ni asignación L/R. Las métricas locales tampoco representan latencia acústica. Mantener abiertas #2, #12, #13, #14, #20 y #22 hasta las pruebas pertinentes.

## Comprobar CPU y estado

1. Con el mismo Windows/PC/receptor, medir CPU estando parado y reproduciendo, anotando el modo de búfer. El bug de #22 era acumulativo: cada sesión NTP descartada podía dejar un núcleo ocupado.
2. Repetir al menos diez ciclos reproducir/parar y diez cambios de receptor. Esperar unos segundos tras cada parada; CPU y número de hilos deben regresar a una referencia estable, sin crecimiento por sesión.
3. Cancelar durante pairing, preparación de captura y cambio de latencia. El botón Cancelar debe funcionar antes de que aparezca un reproductor; una operación antigua no debe reactivar audio después de parar.
4. Cambiar a una IP que no responde mientras otro receptor reproduce. El anterior debe seguir funcionando y la UI conservarlo. Volver a cambiar y comprobar que no quedan heartbeats de la sesión fallida.
5. Con reconexión desactivada, retirar la fuente o la red: se debe terminar la sesión y restaurar el silencio local. Con reconexión activada, comprobar las tres esperas, un retorno exitoso y cancelación inmediata con Parar. Los fallos de autenticación/configuración requieren acción del usuario.

## Salida Windows y silencio

1. Seleccionar una salida distinta del predeterminado estando parado. Reproducir una aplicación en esa salida; AirSend captura su mezcla, sin redirigir aplicaciones.
2. Seleccionar salida predeterminada y cambiarla en Windows durante audio. Comprobar que la captura se reinicia sobre la nueva salida y la anterior recupera su mute. Retirar una salida seleccionada explícitamente debe producir un fallo, sin capturar otro endpoint por sorpresa.
3. Activar Silenciar salida local y arrancar audio continuo. AirSend espera señal, aplica mute y descarta los primeros 150 ms como prueba de señal posterior al cambio. Tras 800 ms acepta mute si continúa señal o lo revierte y muestra la limitación. Una pausa legítima puede hacer que la prueba revierta mute; repetir con audio continuo.
4. Probar salida originalmente silenciada/no silenciada, Parar, fallo de red y Salir desde bandeja. Confirmar restauración. Cambiar volumen/mute manualmente durante la sesión: se conserva el nuevo volumen; un cambio manual del mute cancela la restauración automática de esa sesión.
5. Con mute activo, terminar forzosamente AirSend y abrirlo de nuevo. El fichero `local-mute-recovery.json` en el directorio de datos del identificador `com.pablodiaz.conexionairplay` debe permitir recuperar el mute anterior. Si el endpoint está desconectado, el fichero se conserva y bloquea nuevas sesiones de mute hasta poder restaurarlo. No borrarlo para forzar otra captura.

## Preferencias y grupo

- Cambiar volumen y latencia en un receptor, repetir en otro y volver: ajustes independientes. La latencia de las salidas independientes añadidas debe coincidir con la sesión ya activa. Una lectura no disponible se indica y no se repite cada pocos segundos. Se conserva la escala de ganancia normalizada de AirSend; la lectura dB usa la inversa de su escritura existente.
- Con lectura soportada, cambiar volumen en el receptor y comprobar su control individual; mover rápidamente el slider mientras hay lecturas y verificar que una respuesta anterior no sobrescribe la solicitud más reciente.
- Configurar una pareja en Apple Home y activar Mostrar parejas: una sola fila con ambos miembros; con uno ausente no debe permitir iniciar una pareja completa. Probar L/R con señales diferenciadas.
- Para dos HomePods independientes, usar los selectores de grupo experimental. Escuchar inicio coordinado y comprobar deriva durante al menos 30 minutos. Retirar un miembro y probar la política de parada/recuperación.
- Repetir volumen, latencia, cancelar y parar sobre el grupo; los miembros conservan sus claves independientes y el líder mantiene su orden tras reiniciar.

## Instalador y actualización

Desde el artefacto de la ejecución Windows del PR, verificar el `.exe` con `Get-FileHash -Algorithm SHA256` y compararlo con `SHA256SUMS`. Instalar sobre 0.1.7, comprobar ajustes, identificador, inicio, bandeja y desinstalación. El botón Buscar actualizaciones abre únicamente la release oficial; no instala nada automáticamente. El workflow de release mantiene publicación en borrador y sube los hashes. No promocionar a una release estable hasta comparar las rutas optativas en hardware.
