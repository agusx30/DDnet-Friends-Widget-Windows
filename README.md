# DDNet Friends (widget de bandeja)

Ícono gris en la bandeja del sistema de Windows (junto al reloj) que muestra qué amigos
de DDNet están conectados.

- **Pasar el mouse** sobre el ícono: tooltip con los nombres.
- **Clic** en el ícono: menú con nombre, mapa y servidor de cada amigo, más "Actualizar ahora" y "Salir".
- **Notificación de Windows** cuando un amigo se conecta (no avisa en la primera carga).
- Se actualiza solo cada 45 segundos y relee tu lista de amigos en cada ciclo.
- No necesita token: usa la lista maestra pública `master1.ddnet.org`.

## Compilar (en Windows)

    cargo build --release

Resultado: `target\release\ddnet-friends.exe`

Lee por defecto `%APPDATA%\DDNet\settings_ddnet.cfg`. Ruta alternativa:

    ddnet-friends.exe "C:\ruta\settings_ddnet.cfg"

## Inicio automático

Win+R, escribí `shell:startup` y poné ahí un acceso directo al .exe.

## Notas

- Los amigos se leen de las líneas `add_friend "nombre" "clan"` (el clan es opcional).
- Si el ícono no aparece, Windows lo puede esconder en la flecha "^" de la bandeja:
  arrastralo a la barra para fijarlo.
