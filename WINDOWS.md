# O2 Cloud en Windows

Esta nota recoge la revisión de compatibilidad con Windows, los cambios hechos
y lo que queda por probar a mano. El objetivo no era solo compilar: la app se
había desarrollado y validado en macOS, así que se revisaron las rutas de código
que dependen del sistema operativo.

## Entorno probado

- Windows 11 x86_64.
- Rust 1.99.1 (`x86_64-pc-windows-msvc`) y Visual Studio 2022 Build Tools.
- Node 26.10 y npm 11.19.
- Tauri 2.12.5 y WebView2 Runtime.
- CLI `o2cloud 0.8.0` de [`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli) (`o2cloud-x86_64-pc-windows-msvc.zip`).

> Nota posterior: la versión mínima es 1.2.0 y las releases incluyen 1.4.1.
> Las pruebas de este documento se hicieron con 0.8.0; la actualización y las
> pasadas de humo con 0.12.0 y 1.2.1 están en `PENDING_TESTS.md`; las de
> 1.3.1 y 1.4.1 están pendientes.

Las pruebas se hicieron con una cuenta real de O2 España, además de con un CLI
simulado. Se probaron compilación, tests, arranque, instaladores, descarga del
CLI de `o2cloud-cli`, login por SMS, listado, descarga normal, subida normal, subida y
descarga por trozos con verificación SHA-256, subida de carpetas y la biblioteca
compartida. Los archivos de prueba se borraron al terminar (quedan en la
papelera de O2, sin vaciarla para no tocar otros elementos de la cuenta).

## Cambios de compatibilidad

### Nombres de archivo al descargar

O2 permite nombres que Windows rechaza. Sin sanearlos, una descarga podía fallar
al crear el archivo, o peor, un nombre como `..\algo` podía salirse de la carpeta
elegida.

- `transfer::download_file_name` sustituye `< > : " / \ | ? *`, caracteres de
  control y separadores de ruta.
- En Windows también corrige nombres reservados (`CON`, `PRN`, `AUX`, `NUL`,
  `COM1`–`COM9`, `LPT1`–`LPT9`, incluso con extensión), puntos y espacios
  finales, y recorta nombres largos a 180 unidades UTF-16 conservando la
  extensión.
- `commands::download_entry` aplica el mismo saneado al destino elegido, y el
  diálogo de guardado del frontend propone ya un nombre válido.
- Las partes temporales de una descarga por trozos usan `part-001`, `part-002`…
  en vez del nombre remoto, para que ningún nombre de O2 controle la ruta.

### Guardado atómico de la caché

En Windows `std::fs::rename` no reemplaza un archivo existente. La caché de
listados se guardaba en un `.tmp` y se renombraba encima del archivo anterior:
a partir del segundo guardado, el rename fallaba y la caché dejaba de
actualizarse en disco. Ahora se elimina el destino antes del rename, y el test
`persists_and_reloads_a_listing` guarda dos veces para cubrirlo.

### Progreso de subida y watchdog

macOS y Linux pueden leer el offset del archivo que el CLI tiene abierto. Windows
no expone los handles de otro proceso a un proceso normal, así que la barra se
quedaba sin porcentaje y el watchdog podía no detectar una subida atascada.

- `progress.rs` usa `OpenProcess` + `GetProcessIoCounters` y toma los bytes
  leídos por el proceso del CLI, limitados al tamaño del archivo.
- Es una estimación (el CLI puede leer por delante de la red), pero suficiente
  para mostrar avance y para que el watchdog corte una subida parada.
- El watchdog ya no se desactiva cuando no hay un `FileId`; si la plataforma no
  puede medir, sigue aplicando el límite de espera.

### Ventanas de consola

`version_of` ejecutaba `o2cloud --version` al arrancar y podía hacer parpadear
una consola. Tanto esa comprobación como `run_watched` usan ahora
`CREATE_NO_WINDOW` en Windows. La ventana propia del login del CLI sigue
apareciendo, porque es una ventana del CLI y no una consola.

### Archivos ocultos y de sistema

Al subir una carpeta, la app omite archivos que empiezan por `.`. En Windows
también se omiten los que tienen el atributo *hidden* o *system* (`desktop.ini`,
`Thumbs.db`, etc.), para no subir metadatos del sistema.

### Interfaz

- La app detecta macOS/Windows con el *user agent* del WebView.
- Los atajos muestran `⌘` en macOS y `Ctrl` en Windows; la papelera muestra
  `Supr` y abrir muestra `Intro`.
- Los tooltips de la barra y el panel de atajos de Ajustes se actualizan al
  arrancar, sin duplicar el HTML.
- La selección múltiple ya usaba `metaKey || ctrlKey`, así que funciona igual
  con Ctrl en Windows.

### CLI

- El asset de Windows de `o2cloud-cli` existe y se probó:
  `o2cloud-x86_64-pc-windows-msvc.zip`, versión 0.8.0.
- No se añade el binario de Windows al repositorio. En el primer arranque, si no
  hay CLI, la app abre Ajustes y **Buscar actualización** descarga, extrae y
  prueba el binario publicado por `o2cloud-cli`.
- La extracción `.zip`, la comprobación de `--version` y el guardado en
  `%APPDATA%\es.dabi.o2cloud\cli\` se probaron de extremo a extremo: la app
  descargó el `.zip` de `o2cloud-cli`, extrajo `o2cloud.exe`, escribió
  `current.json` y cambió a `source: managed`.
- `version_of` y las ejecuciones del CLI no muestran consola.
- `tauri.windows.conf.json` deja `bundle.resources` vacío en Windows, para no
  meter el binario de macOS (`bin/macos-aarch64/o2cloud`, 5,5 MB) en el
  instalador. En macOS se sigue incluyendo el binario correspondiente.

### Lo que sigue siendo exclusivo de macOS

- **Subir desde Fotos** usa PhotoKit y `photos.m`; en Windows
  `photos_access_status` devuelve `available: false` y la vista no aparece.
- La firma ad-hoc y el `.dmg` son pasos de `app.sh`; en Windows no aplican.

## Compilar y ejecutar

Requisitos:

1. Node 22 o posterior: `winget install OpenJS.NodeJS.LTS`.
2. Rust con el target MSVC: `winget install Rustlang.Rustup`.
3. Visual Studio 2022 Build Tools con «Desarrollo para el escritorio con C++».
4. WebView2 Runtime (viene con Windows 10/11; si falta, el instalador de Tauri
   puede descargarlo).

Con el script:

```powershell
.\app.ps1 dev        # desarrollo, recarga al cambiar el código
.\app.ps1 build      # .exe de release
.\app.ps1 open       # abre el .exe, compilándolo si falta
.\app.ps1 installer  # .exe + .msi
```

Si PowerShell bloquea el script por la política de ejecución:

```powershell
powershell -ExecutionPolicy Bypass -File .\app.ps1 dev
```

También vale `pwsh` si tienes PowerShell 7.

A mano:

```powershell
npm install
npm run tauri dev
npm run tauri build -- --no-bundle
npm run tauri build
```

Salidas:

```
src-tauri\target\release\o2cloud-app.exe
src-tauri\target\release\bundle\nsis\O2 Cloud_0.1.0_x64-setup.exe
src-tauri\target\release\bundle\msi\O2 Cloud_0.1.0_x64_en-US.msi
```

## Resultados en esta máquina

- [x] `cargo check --all-targets`.
- [x] `cargo test`: 39 tests, 0 fallos (incluye los tests nuevos de Windows y de `status --json`).
- [x] `npm run build` (TypeScript + Vite).
- [x] `npm run tauri build -- --debug --no-bundle`: la app arranca y pinta la
      ventana.
- [x] `o2cloud.exe --version` → `o2cloud 0.8.0`.
- [x] `o2cloud.exe status` → `✗ Not logged in. Run o2cloud login to authenticate.`
- [x] Descubrimiento del CLI con `O2CLOUD_CLI`, con `PATH`/`which` y con el CLI
      gestionado (`source: managed`).
- [x] Descarga e instalación del CLI de `o2cloud-cli` desde GitHub dentro de la app;
      `current.json` y `versions\0.8.0\o2cloud.exe` correctos.
- [x] La ventana de login del CLI se abre en Windows
      («O2 Cloud - Iniciar sesión») y responde.
- [x] Login real por SMS completado en la ventana del CLI; sesión activa y
      cuota leída (`status --json`).
- [x] Listado real de la cuenta: raíz, carpeta de 5.321 archivos, vista virtual
      «Cámara» (46.463 archivos) y sus carpetas por año.
- [x] Descarga real de un archivo de 32,3 MB; tamaño exacto y sin
      `.o2download` residual.
- [x] Subida real de un archivo de 79 B con espacios y `ñ`; descarga posterior
      con el mismo SHA-256; archivo borrado al terminar.
- [x] Subida y descarga reales por trozos: archivo de 300 MB dividido en
      256 MB + 44 MB, manifiesto, verificación SHA-256 correcta y borrado al
      terminar. La subida tardó 49 s y la descarga 38 s.
- [x] Subida real de una carpeta con subcarpeta y archivo oculto: se subieron
      `visible.txt` y `sub\inside.txt`, y se omitió `hidden.txt`; carpeta
      borrada al terminar.
- [x] Navegación, búsqueda, Ajustes, «Buscar actualización» (0.8.0 al día),
      sincronización de la biblioteca y panel de transferencias en la UI.
- [x] Atajos de teclado en Windows: `Ctrl+F` enfoca la búsqueda, `Ctrl+A`
      selecciona todo y `Esc` deselecciona.
- [x] Menú contextual con `Descargar… ↩` y `Mover a la papelera Supr`.
- [x] El diálogo nativo «Guardar archivo» se abre y se cancela sin dejar la
      app bloqueada.
- [x] Tras reiniciar, la app muestra el listado guardado en disco antes de que
      termine el `ls` de la cuenta (caché de 4,7 MB leída correctamente).
- [x] `npm run tauri build` con instaladores NSIS/MSI:
      `O2 Cloud_0.1.0_x64_en-US.msi` (3,55 MiB) y
      `O2 Cloud_0.1.0_x64-setup.exe` (2,55 MiB).
- [x] Instalación silenciosa del `.exe` en una carpeta temporal, arranque del
      `o2cloud-app.exe` instalado y desinstalación limpia.
- [x] La UI se renderiza centrada y completa a 200 % de escalado (comprobado
      con una captura del WebView y `innerWidth`/`devicePixelRatio`).
- [x] No aparece una consola al arrancar ni al ejecutar el CLI.
- [x] Flujos con un CLI simulado: listado, descarga normal, descarga por trozos
      con verificación SHA-256, subida pequeña desde una ruta con acentos y
      espacios, subida por trozos y carpeta con archivo oculto.

### Pruebas con un CLI simulado

Para validar las rutas de Windows sin una cuenta se compiló un `o2cloud.exe`
de prueba que responde a `status`, `ls`, `download`, `upload`, `mkdir` y `rm`.
No forma parte del repositorio. Con él se comprobó:

- `list_dir` y `get_session_status` con una sesión ficticia.
- `download_entry` de un nombre `bad:name?.txt`: se guardó como
  `bad_name_.txt` y el contenido coincidió.
- Descarga normal en una carpeta con espacio y `ñ`.
- Descarga por trozos: manifiesto con dos partes, SHA-256 por parte y del
  archivo completo, partes temporales `part-001`/`part-002`, concatenación y
  borrado del `.o2download`.
- Subida de un archivo con espacios y acentos.
- Subida por trozos de un archivo de 300 MB con umbral de 256 MB: la biblioteca
  registró `grande.bin.o2manifest.json` y no quedaron temporales.
- Subida de una carpeta: se subieron `visible.txt` y `sub\inside.txt`, y se
  omitieron `.dotfile` y `hidden.txt` (atributo *hidden*).

Esto valida el código de la app en Windows, no el comportamiento real de O2 ni
del CLI de `o2cloud-cli` durante una transferencia.

## Pruebas manuales pendientes

- [ ] Subir un archivo mayor que 3,5 GB, cancelar a mitad, reintentar y
      comprobar que reanuda desde los trozos que ya están en O2.
- [ ] Descargar un archivo troceado de más de 3,5 GB y verificar el SHA-256
      (el circuito ya se probó con 300 MB).
- [ ] Cancelar una descarga a mitad y comprobar que no queda el `.o2download`.
- [ ] Cancelar una transferencia desde el panel de transferencias.
- [ ] Descargar un nombre real con `:`, `?`, `*`, `CON.txt`, puntos finales o
      más de 255 caracteres (el saneado se probó con el CLI simulado).
- [ ] Mover algo a la papelera y restaurarlo desde la web de O2.
- [ ] Caducar la sesión y comprobar que la app pide login de nuevo.
- [ ] Probar rutas en OneDrive y una ruta cerca del límite de 260 caracteres.
- [ ] Abrir el `.exe`/`.msi` desde el menú Inicio tras instalarlo (la
      instalación y desinstalación silenciosas ya se probaron).
- [ ] Probar en un Windows 10 y en un Windows sin WebView2 para ver el
      bootstrap del instalador.
- [ ] Comprobar el modo claro y el escalado al 125 %/150 % en una pantalla
      normal (a 200 % la UI ya se vio completa).
- [ ] Arrastrar archivos y carpetas desde el Explorador a la ventana.
- [ ] Usar el diálogo nativo de abrir archivos/carpetas para subir (el de
      guardar ya se abrió y canceló correctamente).
- [ ] Probar la instalación en un equipo limpio, sin Rust ni Node.

## Limitaciones conocidas

- El progreso de subida en Windows es aproximado (bytes leídos por el proceso
  del CLI, no el offset exacto del archivo).
- Los instaladores no están firmados; SmartScreen puede avisar.
- La app no incluye un binario de Windows: la primera vez hay que descargarlo
  desde Ajustes o definir `O2CLOUD_CLI`.
- Las descargas pueden guardarse con un nombre ligeramente distinto si el nombre
  original usa caracteres que Windows prohíbe.
