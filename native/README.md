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
`settings.json` de AirSend en `%APPDATA%`. Un último receptor guardado activa
el flujo de reconexión y reproducción al arrancar.

En **Audio → Avanzado**, **Silenciar salida local** silencia la salida de
Windows seleccionada mientras sigue sonando el altavoz AirPlay. AirSend
restaura el sonido local al parar o si se interrumpe la captura.
**Búfer local reducido** se puede activar con la reproducción detenida.

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
