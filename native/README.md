# AirSend para Windows (WinUI 3)

La aplicación nativa usa WinUI 3 para las vistas y Win32 para el icono de bandeja.
Carga el motor Rust en el mismo proceso. La DLL conserva la captura WASAPI,
el descubrimiento, el emparejamiento y el envío AirPlay. La interfaz intercambia
órdenes y metadatos con Rust, mientras que el audio permanece dentro del motor.

## Compilación

En Windows x64, con el SDK .NET 10, las herramientas de C++ de MSVC y Rust
`x86_64-pc-windows-msvc`, ejecuta desde la raíz:

```powershell
dotnet build native/AirSend.csproj -c Release -p:Platform=x64
```

El proyecto compila y copia `airsend_engine.dll` y el icono junto a
`AirSend.exe` en `native/bin/x64/Release/net10.0-windows10.0.19041.0/win-x64`.
La compilación incluye Windows App SDK. Para distribuir una carpeta que también
incluya el runtime de .NET, usa:

```powershell
dotnet publish native/AirSend.csproj -c Release -p:Platform=x64 --self-contained true -o target/native-publish
```

Cierra la edición anterior antes de probar la nativa. Ambas leen el mismo
`settings.json` de AirSend en `%APPDATA%`. WinUI selecciona el último
receptor guardado al arrancar. El envío empieza al pulsar **Enviar audio del PC**.

En **Audio → Avanzado**, **Silenciar salida local** silencia la salida de
Windows seleccionada mientras sigue sonando el altavoz AirPlay. AirSend
restaura el sonido local al parar o si se interrumpe la captura.
**Búfer local reducido** se puede activar con la reproducción detenida.

**Liberar el altavoz durante el silencio** viene desactivado. Para probarlo,
actívalo en **Audio → Avanzado** y elige un minuto. Con el HomePod reproduciendo
desde AirSend, deja en silencio la fuente hasta ver **En espera**. Inicia
AirPlay desde el iPhone y vuelve a reproducir audio en el PC. El iPhone debe
seguir sonando hasta que decidas pulsar **Reanudar**. Comprueba que **Parar**
cancela la espera y que al reiniciar WinUI el altavoz sigue en manos del iPhone.

### Captura por aplicación

En Windows 11, abre **Audio**, pon a sonar una aplicación y pulsa **Actualizar**.
Elige la aplicación en **Audio que quieres enviar**. **Todo el sistema** vuelve a
capturar la mezcla de la salida de Windows seleccionada. La selección de salida
y el silencio local se aplican a esa mezcla.

Con Spotify, Chrome o Edge y VLC o un reproductor local, selecciona cada uno
por separado y confirma que solo ese programa se oye en el HomePod. Deja
Discord sonando durante la prueba para comprobar que su audio queda fuera.
En el navegador, prueba dos pestañas y varias ventanas. Cierra y vuelve a abrir
la aplicación elegida mientras AirSend envía. Si la sesión sigue abierta,
AirSend espera a que vuelva la misma aplicación. Si el temporizador ya liberó
el altavoz, vuelve a ponerla a sonar y pulsa **Reanudar**. La selección se guarda
por ruta del ejecutable, así que si una actualización cambia la ruta, vuelve
a elegir la aplicación.

## Validación con HomePod

- Busca receptores, conecta uno, inicia la reproducción y cambia el volumen.
- Cierra la ventana con **X**, verifica que el sonido continúa y vuelve a abrirla
  desde la bandeja. **Salir** debe detener el envío y terminar el proceso.
- Prueba la entrada manual con IPv4, con puerto explícito y con IPv6 entre
  corchetes. Verifica que los anuncios AirPlay y RAOP del mismo receptor aparecen
  agrupados y que se usa una ruta alternativa si falla la principal.
- Con dos receptores, prueba añadir y retirar una salida, cambiar de receptor
  con confirmación y desactivar la reproducción múltiple.
- Comprueba que el búfer empieza en 100 ms, cambia en pasos de 50 ms y migra
  los antiguos valores de 0 ms a 100 ms. Durante la reproducción, verifica el
  reinicio y el intervalo de 10 segundos entre cambios.
- En Audio, enumera y selecciona una salida WASAPI y prueba la temporización
  antigua, el silencio local y el búfer local reducido. Comprueba que el
  silencio se restaura al parar o si se pierde la señal.
- Prueba una pareja de Apple Home y dos HomePods independientes con el envío
  sincronizado. Verifica el volumen por salida y la reconexión tras un fallo.
- Cambia de idioma y exporta el diagnóstico tras al menos 10 segundos de
  captura. El JSON contiene métricas numéricas y excluye datos del receptor.
- Compara en compilaciones Release la memoria privada residente y la CPU en
  reposo, en bandeja y transmitiendo al mismo receptor. Cuenta los procesos
  WebView2 de la edición anterior. Toma varias muestras una vez estabilizada
  cada medición.

La edición Tauri sigue disponible para comparar y validar paridad durante la
migración. El flujo de publicación existente aún empaqueta esa edición.

### Medición de sincronía PTP y estéreo

1. En WinUI, reproduce dos minutos con un HomePod que negocie PTP. Exporta el
   informe desde **Diagnóstico** durante la reproducción. En la salida del JSON,
   comprueba `timing_protocol` y que `ptp_reference.updates_applied` avance
   entre dos informes. Si el receptor usa NTP, `ptp_reference` será nulo.
2. Repite con un par estéreo creado en Casa y, en una prueba aparte, con dos
   salidas independientes. En el modo de grupo, `transport_scope: "group"`
   indica que las dos filas de salida comparten la misma referencia PTP y
   línea RTP. Los cambios de offset miden la referencia que aplica el emisor.
   `rtp_sync_scheduled` cuenta decisiones de programar sincronización y
   `rtp_timeline_catchups` cuenta los saltos para recuperar tiempo de captura.
3. Para medir el desfase acústico, usa dos micrófonos conectados a la misma
   interfaz y colocados a la misma distancia de cada altavoz. Reproduce impulsos
   de referencia en ambos canales y mide la diferencia L/R entre sus llegadas.
   Anota la mediana y el p95 del desfase absoluto, la deriva en ms/min y los
   cortes durante el arranque, la reproducción sostenida y la reconexión de
   un miembro. Usa la misma colocación, volumen y búfer al comparar versiones.
