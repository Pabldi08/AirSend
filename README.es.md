<h1 align="center">AirSend</h1>

<p align="center">Envía el audio de tu PC con Windows a un HomePod por AirPlay 2.</p>

<p align="center">
  <a href="https://github.com/Pabldi08/AirSend/releases/latest"><img src="https://img.shields.io/badge/Descargar-Windows%20x64-blue?style=for-the-badge" alt="Descargar AirSend para Windows x64"></a>
</p>

<p align="center"><a href="README.md">English</a> · <a href="README.es.md">Español</a></p>

## Versión actual

**AirSend 0.3.0** incluye la interfaz nativa WinUI y un instalador para Windows x64. AirSend es gratuito y de código abierto.

1. Descarga y ejecuta el instalador **Setup-x64.exe** desde [GitHub Releases](https://github.com/Pabldi08/AirSend/releases/latest). Si AirSend está abierto, sal primero desde la bandeja.
2. Abre AirSend, elige tu HomePod y pulsa **Enviar audio del PC**.

## Qué puedes hacer

| Función | Cómo funciona |
| --- | --- |
| Audio del sistema o de una app | Envía toda la mezcla del sistema o elige una aplicación en Windows 11, como Chrome o Spotify. |
| Detección de altavoces | Busca altavoces en la red local o introduce una dirección IP manualmente. |
| Reproducción sincronizada | Usa una pareja estéreo o dos HomePods compatibles con un reloj PTP compartido. |
| Controles de reproducción | Ajusta el volumen y solicita al altavoz un búfer de **100 a 3000 ms**. El retraso audible también depende del receptor y la red. |
| Silencio local y espera | Silencia la reproducción local cuando sea compatible. Puedes liberar el altavoz tras un periodo de silencio y reanudar cuando quieras. |
| Bandeja del sistema | Cierra la ventana para seguir escuchando, vuelve a abrirla desde la bandeja o detén la reproducción desde su menú. |
| Diagnóstico | Consulta incidencias y retransmisiones, marca la distorsión audible y exporta un informe sin nombres de altavoces, direcciones de red ni credenciales. |

La app de Windows usa **WinUI 3**, un motor en **Rust** y captura mediante **WASAPI**. Consulta la [guía de Windows](native/README.md) para compilar y probar con hardware.

## Comparativa con TuneBlade y AirParrot

| Función | AirSend | TuneBlade | AirParrot 3 |
| --- | --- | --- | --- |
| Plataforma de envío | Windows x64 | Windows | Windows y macOS |
| Fuente de audio | Todo el sistema o una aplicación en Windows 11 | Audio de todo el sistema | Audio del sistema, archivos multimedia y duplicación de una aplicación en Windows |
| HomePod | Audio por AirPlay 2, con reloj PTP compartido para parejas compatibles | Compatibilidad por firmware sin verificar en esta comparativa | Audio para HomePod documentado en Windows y macOS |
| Control del búfer | Búfer solicitado al receptor de 100 a 3000 ms | — | — |
| Diagnóstico | Incidencias recientes, retransmisiones y marcas de distorsión exportables | — | — |
| Duplicación de pantalla | Solo audio | Envío de audio | Duplicación de pantalla y de aplicaciones |

Los guiones indican funciones sin verificar para esta comparativa. Fuentes de referencia. [Documentación de la integración de TuneBlade](https://github.com/spycle/tuneblade/blob/main/README.md) · [Funciones de AirParrot 3](https://blog.airsquirrels.com/introducing-airparrot-3-the-most-powerful-screen-mirroring-and-media-streaming-software-yet) · [Notas de AirParrot para Windows](https://www.airsquirrels.com/airparrot/release-notes/win)

## Comunicar un fallo

[Abre una issue en GitHub](https://github.com/Pabldi08/AirSend/issues/new) con tu versión de AirSend y Windows, el modelo y software del altavoz y los pasos para reproducir el fallo. Si oyes distorsión, pulsa **Marcar distorsión** en ese momento y adjunta el informe que puedes exportar desde **Diagnóstico**.

## Contribuir

Consulta las notas para mantenedores en [CONTRIBUTING.md](CONTRIBUTING.md).

## Licencia

AirSend es software libre publicado bajo la
[GNU General Public License v3.0 o posterior](LICENSE).

La copia de `airplay2-rs` incluida usa el mismo identificador SPDX,
`GPL-3.0-or-later`. Su revisión exacta, el origen de la rama y la base
de esa licencia se documentan en [PROVENANCE.json](vendor/airplay2-rs/PROVENANCE.json).

El proyecto incorpora y modifica código de
[`lmcgartland/airplay2-rs`](https://github.com/lmcgartland/airplay2-rs),
que también está publicado bajo GPL-3.0-or-later. Consulta el archivo
`LICENSE` para conocer los términos completos.

## Apoya el proyecto

Si encuentras AirSend útil puedes apoyar el proyecto aquí:

<a href="https://ko-fi.com/E3E226XAYR" target="_blank" rel="noopener noreferrer">
  <img
    src="https://storage.ko-fi.com/cdn/kofi3.png?v=6"
    alt="Buy Me a Coffee"
    width="140"
    height="36"
  />
</a>
