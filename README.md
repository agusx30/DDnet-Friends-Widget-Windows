# DDNet Friends (widget de bandeja)

Widget en la bandeja del sistema de Windows (junto al reloj) que muestra qué amigos de DDNet están conectados.

- **Ícono dinámico:**
  - **Celeste:** cuando hay amigos conectados.
  - **Gris:** cuando no hay nadie conectado.
- **Pasar el mouse** sobre el ícono: tooltip que muestra directamente la lista de nombres conectados (sin prefijos y aprovechando el búfer extendido de Windows).
- **Clic / Clic derecho** en el ícono: menú desplegable con:
  - Lista de amigos conectados: nombre, clan, mapa y servidor.
  - **Clic en un amigo:** te une automáticamente a su servidor lanzando el cliente de DDNet.
  - **Actualizar ahora:** fuerza la consulta a la lista maestra sin esperar el intervalo.
  - **Configurar/Cambiar ruta de DDNet.exe:** abre un selector nativo de archivos para indicar dónde está instalado el juego (se autodetecta en el registro o Steam si existe).
  - **Salir:** cierra la aplicación.
- **Notificación de Windows** cuando un amigo se conecta (no avisa en la primera carga).
- Se actualiza automáticamente cada 45 segundos y relee tu lista de amigos en cada ciclo.
- No necesita token: consulta la lista maestra pública `master1.ddnet.org`.

## Compilar (en Windows)

```powershell
cargo build --release
```

Resultado: `target\release\ddnet-friends.exe`

Por defecto lee los amigos de `%APPDATA%\DDNet\settings_ddnet.cfg`. Si usás una ruta alternativa:

```powershell
ddnet-friends.exe "C:\ruta\settings_ddnet.cfg"
```

La configuración del ejecutable del juego se guarda en `%APPDATA%\ddnet-friends\config.json`.

## Inicio automático

Presioná `Win + R`, escribí `shell:startup` y poné ahí un acceso directo a `ddnet-friends.exe`.

## Notas

- Los amigos se leen de las líneas `add_friend "nombre" "clan"` (el clan es opcional).
- Si el ícono no aparece visible en la barra, Windows lo puede esconder en el menú desplegable "^" de la bandeja: arrastralo a la barra para fijarlo junto al reloj.
