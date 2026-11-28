# O2 Cloud en Linux (WSL2)

Esta nota recoge la prueba de la app en Linux x86_64 mediante **WSL2 + WSLg**.
No sustituye a una prueba en un escritorio Linux nativo, pero sirve para
comprobar que el código compila, que los tests pasan y que la interfaz Tauri se
abre y funciona fuera de macOS/Windows.

## Entorno probado

- Windows 11 x86_64 con WSL2 2.1.5.
- Ubuntu 24.04.1 LTS y WSLg 1.0.60.
- Rust 1.99.1 (`x86_64-unknown-linux-gnu`) instalado con rustup.
- Node 22.23.4 y npm 10.9.9.
- WebKitGTK 4.1 y GTK 3.
- CLI `o2cloud 0.8.0` de [`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli)
  compilado desde el código fuente (ver más abajo). Las pruebas de este
  documento se hicieron con 0.8.0; la versión mínima actual es 1.2.0 y la
  incluida en las releases es 1.4.3. El asset oficial pide GLIBC 2.39
  (Ubuntu 24.04 o más nuevo); la app compila 1.4.3 desde fuente en Ubuntu
  22.04 para mantener el soporte de 22.04+.

## Resultado

- `cargo check --all-targets`: correcto.
- `cargo test`: **38 tests, 0 fallos**. En Linux no se ejecuta el test
  específico de Windows, por eso son 38 y no 39.
- `npm run build`: correcto.
- `npm run tauri build -- --debug --no-bundle`: correcto.
- La app se abre bajo WSLg y carga una cuenta real **en modo lectura**: raíz,
  carpetas, búsqueda y navegación por teclado.
- «Subir desde Fotos» no está disponible en Linux, igual que en Windows.
- No se probaron subidas, descargas ni borrados en Linux; esos flujos se
  validaron en Windows y el código es común.

## Dependencias

En Ubuntu 24.04:

```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  build-essential \
  curl \
  wget \
  file \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  pkg-config \
  patchelf
```

Rust, sin `sudo`:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --profile minimal
source "$HOME/.cargo/env"
```

## Compilar la app

Conviene trabajar en el sistema de archivos de Linux (`~/`), no en `/mnt/c`:
las compilaciones sobre el sistema de archivos de Windows son mucho más lentas.

```bash
git clone https://github.com/dcarnicer/o2cloud-app.git ~/o2cloud-app-linux
cd ~/o2cloud-app-linux
source "$HOME/.cargo/env"
npm install
npm run tauri build -- --debug --no-bundle
```

El binario queda en:

```
~/o2cloud-app-linux/src-tauri/target/debug/o2cloud-app
```

Para abrirlo desde una sesión con WSLg:

```bash
cd ~/o2cloud-app-linux
source "$HOME/.cargo/env"
O2CLOUD_CLI="$HOME/o2cloud-app-linux/src-tauri/bin/linux-x86_64/o2cloud" \
  ./src-tauri/target/debug/o2cloud-app
```

No fuerces `GDK_BACKEND=wayland` ni `GDK_BACKEND=x11`: en WSLg, GTK elige el
backend que funciona (X11) si se deja el valor por defecto. Forzar Wayland falla
porque el socket de WSLg no está en `XDG_RUNTIME_DIR`, y forzar X11 puede fallar
al crear la ventana.

## CLI de Linux

El asset publicado para Linux en `o2cloud-cli` v0.8.0–v0.12.0
(`o2cloud-x86_64-unknown-linux-gnu.tar.gz`) **no arrancaba en Ubuntu 24.04**:
pedía `GLIBC_2.43` y Ubuntu 24.04 trae `2.39`. Desde la 0.13 se compila en
Ubuntu 24.04 y pide `GLIBC_2.39`; en sistemas más antiguos, compílalo:

```
o2cloud: /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.43' not found
```

Es un problema del empaquetado del release, no de la app. La solución es
compilar el CLI desde el código fuente:

```bash
git clone --depth 1 --branch v1.4.3 \
  https://github.com/danielperez9430/o2cloud-cli.git ~/o2cloud-cli
cd ~/o2cloud-cli
source "$HOME/.cargo/env"
cargo build --release

mkdir -p ~/o2cloud-app-linux/src-tauri/bin/linux-x86_64
cp target/release/o2cloud \
  ~/o2cloud-app-linux/src-tauri/bin/linux-x86_64/o2cloud
chmod +x ~/o2cloud-app-linux/src-tauri/bin/linux-x86_64/o2cloud

~/o2cloud-app-linux/src-tauri/bin/linux-x86_64/o2cloud --version
```

Con el binario en `bin/linux-x86_64/o2cloud`, el bundle de Linux lo incluye y la
app lo encuentra sin definir `O2CLOUD_CLI`.

## Login bajo WSLg

La ventana de login del CLI **no funciona en WSLg**:

- En Wayland, `winit` falla con `Could not find wayland compositor`.
- En X11, tras instalar `libxkbcommon-x11-0`, falla con
  `GDK has not been initialized`.

La app Tauri sí funciona; el problema está en la ventana del CLI. Para probar la
cuenta en WSL se puede usar una sesión ya iniciada en otro equipo:

```bash
mkdir -p ~/.config/o2cli
cp /mnt/c/Users/<usuario>/AppData/Roaming/o2cli/auth.json \
  ~/.config/o2cli/auth.json
chmod 600 ~/.config/o2cli/auth.json
```

`auth.json` contiene un token de sesión: bórralo cuando termines de probar. En
un Linux nativo, `o2cloud login` debería abrir su ventana con normalidad.

## Rendimiento y avisos

- El primer `ls --json` sobre una cuenta de ~46.000 archivos tardó unos
  **2,5 minutos** en WSL2. En Windows fue más rápido; en un Linux nativo debería
  acercarse a los tiempos de macOS. Después, la app reutiliza la caché de
  listados.
- WSLg mostró avisos de `libEGL`/DRI3 y `failed to open /dev/dri/renderD128`.
  La app siguió renderizando con la ruta de software. En un escritorio con GPU
  no deberían aparecer.
- La ventana se capturó con X11 (`import -window …`) para comprobar que se
  pintaba correctamente.

## Pendiente

- Probar en un escritorio Linux nativo (GNOME/KDE, Wayland y X11).
- Completar el login del CLI en Linux.
- Probar subida, descarga, borrado y troceado en Linux.
- Generar bundles `.deb`/AppImage y revisar iconos/recursos.
- Probar el asset oficial 1.4.3 en Ubuntu 24.04 (GLIBC 2.39) y el build desde
  fuente en Ubuntu 22.04.
