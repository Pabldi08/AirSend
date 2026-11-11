# Pruebas de AirSend en casa

La rama `issue/validacion-conjunta` reúne las correcciones de las PR [#26](https://github.com/Pabldi08/AirSend/pull/26) y [#27](https://github.com/Pabldi08/AirSend/pull/27), además de las funciones ya integradas en main. Las PR de cada issue siguen separadas; esta rama sirve para probarlas con un solo instalador.

## Descargar e instalar

1. Abre las [ejecuciones Windows de esta rama](https://github.com/Pabldi08/AirSend/actions/workflows/ci.yml?query=branch%3Aissue%2Fvalidacion-conjunta) y elige la última ejecución completada correctamente. Si sigue en curso, espera a que termine.
2. En **Artifacts**, descarga `AirSend-Windows-test-<commit>`. Debes iniciar sesión en GitHub. Descomprime el ZIP: contiene el instalador `.exe` y `SHA256SUMS`.
3. En PowerShell, dentro de la carpeta descomprimida, compara el hash con el listado:

   ```powershell
   Get-ChildItem *.exe | Get-FileHash -Algorithm SHA256
   Get-Content .\SHA256SUMS
   ```

4. Sal de AirSend desde el icono de la bandeja e instala el `.exe`. No desinstales ni borres la configuración: queremos comprobar que se conservan los ajustes.
5. La versión visible puede seguir siendo **0.1.7**. Identifica la compilación por el commit del artefacto y por `build_revision` en **Diagnóstico de audio → Descargar diagnóstico**.

## Preparación — 5 minutos

- Anota versión de Windows, modelo y versión de software de cada receptor y salida de audio del PC.
- Empieza con un único receptor, volumen al 20 %, límite de búfer de 1.000 ms y las opciones experimentales desactivadas. Conecta, pulsa **Enviar audio del PC** y reproduce una canción o vídeo durante dos minutos.
- Confirma que hay sonido y que Parar funciona. Si falla esta prueba básica, descarga el diagnóstico antes de seguir.
- Para cambiar el límite, mueve el control y pulsa **Confirmar**. Espera los 10 segundos entre cambios.

## Pruebas por issue

### #24 — HomePod OS 27, inicio en silencio y pausas

1. Con el HomePod sin reproducir desde otro dispositivo, conecta y envía audio. Debe conectar y sonar; no debería fallar silenciosamente ni quedarse esperando SETUP.
2. Si falla una conexión, comprueba que el error sigue visible después de que los botones vuelvan a estar disponibles y se actualice la lista de dispositivos.
3. Para, activa **Búfer local reducido (experimental)** y selecciona 100 ms. Con el PC en silencio, inicia el envío; espera diez segundos y luego reproduce audio. Debe empezar a sonar sin tener que parar y enviar otra vez.
4. Pausa el vídeo o la música entre tres y cinco minutos y reanuda. El sonido debe volver sin reconectar. Repite dos veces.
5. Repite la prueba de inicio y pausa con 1.000 ms y con el búfer local reducido desactivado. Anota cualquier diferencia entre modos.

### #23 — Búfer mínimo y ajustes antiguos

1. Comprueba que el mínimo seleccionable es 100 ms, que no aparece 0 y que 100, 200, 1.000 y 3.000 ms permiten reproducir. No se espera que la demora audible coincida exactamente con ese número.
2. Si tu instalación anterior tenía guardados 0 ms, al abrir esta compilación deben aparecer 100 ms. Comprueba que conserva volumen y reconexión por receptor. Si no tenías 0 guardado, marca la migración como no probada; no necesitas editar archivos.
3. Cierra desde la bandeja y vuelve a abrir: deben conservarse tus ajustes. Si 100 ms da cortes pero 1.000 ms funciona, registra ambos resultados.

### #20 y #22 — Cambiar de receptor y vigilar CPU

Necesitas dos receptores. Desactiva **Reproducir en varios dispositivos**.

1. Abre el Administrador de tareas y anota CPU de AirSend parado y reproduciendo. En Detalles puedes activar la columna de subprocesos para comparar también el número de hilos.
2. Reproduce en A y conecta B; acepta el cambio. El sonido debe pasar a B, parar en A y coincidir con el receptor que muestra la app.
3. Alterna A/B diez veces. Después haz diez ciclos Enviar/Parar. Tras cada parada espera entre cinco y diez segundos.
4. La CPU y los hilos deben volver aproximadamente a su referencia, sin crecimiento acumulativo por cambio. No se exige CPU exactamente a cero.
5. Cancela una conexión mientras se prepara: no debe empezar audio después de cancelarla. Si puedes provocar un intento fallido hacia otro receptor mientras A reproduce, A debe seguir sonando y la UI debe conservarlo.

### #12 — Silenciar el PC y restaurarlo

1. Con música continua en los altavoces del PC, activa **Silenciar salida local (experimental)** antes de enviar al HomePod. Tras la comprobación inicial, debe sonar el HomePod y dejar de sonar el PC.
2. Para AirSend: el PC debe recuperar su estado de silencio anterior. Repite empezando con la salida ya silenciada.
3. Repite el envío y sal desde la bandeja: también debe restaurarse el estado anterior.
4. Selecciona otra **Fuente de audio de Windows** estando parado y reproduce una aplicación en esa salida. AirSend debe capturar esa salida; no mueve automáticamente las aplicaciones entre salidas.
5. Si el driver deja de entregar audio al silenciar, la app debe revertir el mute e indicar la limitación. Regístralo con el nombre de la salida; no equivale a que el silencio exclusivo funcione en ese driver.

Prueba adicional opcional: con mute activo, termina AirSend desde el Administrador de tareas y vuelve a abrirlo; debe recuperar el estado previo. Si la salida está desconectada, vuelve a conectarla antes de evaluar la restauración.

### #2 — Reproducción en varios dispositivos

1. Activa **Reproducir en varios dispositivos**, envía a A y conecta B.
2. Ambos deben sonar. Desconecta uno: el otro debe seguir reproduciendo.
3. Comprueba los controles de volumen individuales. Este modo mantiene sesiones independientes: registra el desfase, pero la sincronización se comprueba con el modo siguiente.

### #14 — Grupo sincronizado y pareja estéreo

Necesitas dos HomePods; prueba cada modalidad solo si tienes la configuración correspondiente.

1. Para todo. En **Parejas y sincronización (experimental)** elige dos HomePods distintos y pulsa **Probar reproducción sincronizada**.
2. Escucha el arranque y deja música durante 30 minutos. Anota si aparece eco o aumenta el desfase. Una escucha subjetiva no mide la diferencia exacta.
3. Retira un miembro de la red: debe detenerse el grupo completo. Si activas reconexión, comprueba la recuperación del grupo cuando vuelva.
4. Si tienes una pareja creada en Apple Home, activa **Mostrar parejas configuradas en Apple Home**. Debe aparecer una sola fila con ambos miembros. Reproduce un archivo de prueba que anuncie Left/Right y comprueba cada canal; con un miembro ausente no debería iniciar una pareja incompleta.

### #13 — Denon AVR X4800H

Solo se puede verificar si tienes acceso a este receptor; si no, marca **sin hardware**.

1. Confirma que aparece una sola entrada del Denon, conecta y envía audio con 1.000 ms.
2. Comprueba sonido, volumen, Parar y una segunda conexión. Si falla, anota si ocurre al conectar o al enviar, el error exacto y si el Denon pide contraseña o autorización.
3. No consideres resuelto el soporte solo porque aparezca en la lista o permita conectar: tiene que reproducir audio.

## Qué resultados guardar

Después de un fallo, descarga el diagnóstico antes de cerrar o hacer muchos intentos nuevos. Para CPU añade una captura del Administrador de tareas antes y después de los ciclos. Los logs de Windows están en `%APPDATA%\ConexionAirPlay\logs` si hace falta investigar un error que el diagnóstico no explique.

Puedes devolverme los resultados con este formato:

```text
Compilación / build_revision:
Windows:
Receptor(es), modelo y versión:
Salida de audio Windows:

#24 Inicio en silencio y pausa larga: OK / falla / no probado
#23 Búfer mínimo y ajustes: OK / falla / no probado
#20 Cambio A/B: OK / falla / sin segundo receptor
#22 CPU antes / después de 10 cambios y 10 paradas:
#12 Mute y restauración: OK / falla / limitación del driver
#2 Varios dispositivos: OK / falla / sin segundo receptor
#14 Grupo / pareja / 30 minutos: resultado o sin hardware
#13 Denon: OK / falla / sin hardware

Búfer solicitado y modo local en el fallo:
Pasos para repetirlo y error exacto:
Diagnóstico adjunto:
```

Marca como no probado lo que no puedas comprobar. No hace falta disponer de todos los receptores para que el resto de pruebas sea útil.
