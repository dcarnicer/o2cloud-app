# Pruebas pendientes

Estado a 19 nov 2026. Entorno probado: macOS 27, Apple Silicon, una sola máquina.
La sincronización real entre dos Macs queda pendiente por falta de un segundo equipo;
el merge entre dispositivos se ha simulado con un snapshot y un `device_id` distintos.

## Ya validado (contexto)

- [x] Conflicto al subir a la raíz un archivo que ya estaba en otra carpeta.
- [x] Opción **Subir copias**: dos copias en O2 y entrada en la biblioteca.
- [x] Merge de un snapshot de un segundo dispositivo simulado (2 entradas / 2 dispositivos).
- [x] Reconciliación: poda de una copia fantasma y republicación del snapshot limpio.
- [x] Vista de Fotos: permiso, listado de 2.617 vídeos, badges y contador de ya subidos.
- [x] Creación y subida de snapshots a `/.o2cloud-gui/library/`.
- [x] Subida real desde Fotos, incluyendo un original que solo estaba en iCloud.
- [x] Subida de una carpeta con `.txt`, imagen y subcarpeta; estructura, tamaños y hashes.
- [x] Conflicto por archivo: dos duplicados, se sube solo el marcado.
- [x] Borrar `.o2cloud-gui` y comprobar que la app la recrea con una subida nueva.
- [x] Reconciliar desde la UI tras borrar copias en O2.
- [x] Periodo de gracia de la poda: las entradas recientes se conservan y las antiguas se podan.
- [x] Conflicto por archivo con dos vídeos de Fotos y con un PNG local.
- [x] Caché en disco del listado: raíz visible en 2 s con aviso de actualización.
- [x] Snapshots por delta y compactación: dos deltas y un snapshot completo final.
- [x] Olvidar biblioteca con snapshot real y reconstruirla con una subida nueva.
- [x] Indexar manifiestos antiguos: 3 vídeos grandes recuperados por hash.
- [x] Hash preflight opcional de archivos no multimedia y duplicados renombrados.
- [x] Filtros de Fotos por álbum, favoritos y rango de fechas.
- [x] Paginación de Fotos con páginas de 300 y «Mostrar más».
- [x] Miniaturas de Fotos bajo demanda en la lista.

## Pendiente

### 1. Subida real desde Fotos (UI) — validada 28 oct 2026

- [x] Abrir la app y conceder permiso de Fotos si macOS lo pide.
- [x] Abrir «＋ Subir» → «Desde Fotos…».
- [x] Elegir un vídeo pequeño de prueba y seleccionarlo.
- [x] Pulsar «Subir seleccionados».
- [x] Verificar:
  - [x] aparece en Transferencias y termina en «Subido».
  - [x] el archivo está en la carpeta destino de O2.
  - [x] la biblioteca local registra una fuente `photos` con `captured_at`.
  - [x] se sube un snapshot nuevo a `.o2cloud-gui/library`.

Resultado: `ScreenRecording_10-25-2026 22-29-20_1.mp4`, 33.850.613 bytes,
subido a `/__Test3__` y con el mismo tamaño en O2 (id 1551013127).

Notas:

- Tras recompilar la app, macOS puede volver a pedir el permiso de Fotos. Es una
  limitación de la firma ad-hoc, no un fallo de la app. `app.sh` firma el bundle
  completo para que TCC pueda atribuir el permiso.
- El diálogo de privacidad de macOS no acepta clics ni teclas sintéticas: hay que
  concederlo manualmente.
- No usar un vídeo personal sin querer subirlo de verdad.

### 2. Original solo en iCloud — validada 28 oct 2026

- [x] Elegir un vídeo que no esté descargado localmente.
- [x] Subirlo desde Fotos.
- [x] Verificar que la app espera la descarga de iCloud y termina correctamente.
- [x] Comprobar el tamaño y que el original llega a O2.
- [x] Comprobar el espacio libre: se necesitaba aproximadamente su tamaño libre.

Resultado: `99BC9A63-3F56-4A44-88C7-196EE3F16196.mp4`, 21.823.721 bytes.
No estaba entre los originales locales de la fototeca. La descarga fue rápida y
no dio tiempo a capturar la fase «Descargando de Fotos…», pero el archivo llegó
completo y con el tamaño correcto.

### 3. Carpetas y archivos no multimedia — validada

- [x] Crear una carpeta local pequeña con un `.txt` y una foto o vídeo.
- [x] Subirla con «＋ Subir» → «Carpeta…».
- [x] Verificar la estructura remota.
- [x] Confirmar que las carpetas siguen usando solo nombre + tamaño en la carpeta
      destino, sin preflight de hash: una segunda subida de la misma carpeta se
      omitió por nombre+tamaño y el contenido remoto conservó la versión antigua.
- [x] Subir un archivo no multimedia individual y comprobar que su hash se registra
      después: `nuevo.txt` (41 B) quedó con `sha256: 2100e7e3…` y
      `source: "file"` en la biblioteca.
- [x] Hash preflight opcional de no multimedia: con «Hashear otros archivos
      antes de subir» activado, `original.txt` (1.800 B, `f31dcd34…`) subido a
      `/__HashTestA__` se detectó como `renamed.txt` al intentar subirlo a
      `/__HashTestB__`; el diálogo mostró la copia de A y «Saltar» no subió
      nada. Sin la opción se mantiene la comprobación por nombre+tamaño.

Resultado de la carpeta original: `/__Prueba3__/o2-prueba/` con `nota.txt`,
`icono.png` y `sub/otra.txt`; tamaños y hashes coinciden. La biblioteca registró
los tres como `source: "file"`. En la repetición,
`/__FolderTest__/o2-recreate/recrear.txt` se omitió por nombre+tamaño y
`nuevo.txt` individual registró su hash correctamente.

### 4. Sincronización real entre dos Macs

- [ ] En el Mac A, subir un archivo y sincronizar la biblioteca.
- [ ] En el Mac B, iniciar sesión y sincronizar.
- [ ] Comprobar que aparece el snapshot de A y que el `device_id` es distinto.
- [ ] Abrir Fotos en B y verificar que el archivo coincide por huella/hash.
- [ ] Intentar subir el mismo archivo en B y comprobar el diálogo de conflicto.
- [ ] Comprobar que los snapshots de ambos dispositivos se conservan.

### 5. Casos de biblioteca

- [x] Probar «Saltar duplicados» con un caso real de Fotos.
- [x] Borrar copias en O2 y ejecutar «Reconciliar con O2» desde la UI.
- [x] Borrar `.o2cloud-gui` y comprobar que la app la recrea con una subida nueva.
- [x] Snapshots incrementales/compactación: cada lote sube un delta; tras 8
      snapshots propios se sube uno completo y se borran los antiguos. Los IDs
      antiguos también se limpian del mapa local. Probados dos deltas (746 y
      747 B) y la compactación a un snapshot de 1.370 B. Falta validarlo con
      dos equipos.
- [x] Olvidar/reconstruir la biblioteca: probado con un snapshot real. Olvidar
      dejó 0 entradas, `folder_id: null` y la carpeta remota vacía; una subida
      nueva recreó el snapshot y la entrada.
- [x] Elección por archivo en el conflicto: probado con dos vídeos de Fotos y
      con un PNG local; se subió solo el marcado.
- [x] Indexar hashes de manifiestos `.o2manifest.json` antiguos: encontrados
      `IMG_0112.MOV` (6,58 GB), `IMG_6114.MOV` (7,23 GB) y `IMG_4087.MOV`
      (4,40 GB) en `/Vídeos grandes`, con `source: "manifest"` y snapshot nuevo.
- [ ] Concurrencia: dos dispositivos subiendo a la vez (requiere dos equipos).
- [x] Periodo de gracia de la poda y reloj desajustado: probado con una copia
      reciente, otra con fecha futura y otra antigua; las dos primeras se
      conservan y la antigua se poda.
- [ ] Limpieza de snapshots antiguos en la papelera de O2: el CLI no puede
      vaciarla; queda como paso manual desde la web.

### 6. Fotos: pulido

- [x] Miniaturas: 64 px bajo demanda para las filas visibles, con el icono
      como respaldo si PhotoKit no tiene una miniatura local.
- [x] Cancelar una descarga de iCloud: botón «Cancelar» en la fila mientras
      PhotoKit exporta. No se pudo ver la fase con un original grande porque
      los probados ya estaban en caché; falta una prueba con descarga real.
- [x] Comprobar espacio libre antes de descargar un original: la app exige el
      tamaño del vídeo más 128 MB y falla la transferencia con un mensaje claro
      si no hay sitio. No se ha forzado un disco lleno en la prueba real.
- [x] Filtros por álbum, favoritos y rango de fechas: «Berlín» mostró 0 vídeos,
      «Solo favoritos» 9, y del 25/10/2026 al 25/10/2026 quedaron 3 vídeos
      (180 MB, 1 ya en O2). Al abrir Fotos, el rango se precarga del 29/09/2026
      al 29/10/2026; la casilla «Por fecha» filtra a 14 vídeos y «Limpiar»
      vuelve a los 300 de 2.619.
- [x] Paginación real: páginas de 300 desde PhotoKit; probado 300 → 600 con
      2.619 vídeos. Con «Fotos y vídeos» (52.017 elementos) se forzó una página
      de 10.100 y «Mostrar más» subió a 20.200 de 52.017, así que el offset
      >10.000 funciona. La carga inicial tarda ~1-2 min porque PhotoKit recorre
      toda la biblioteca para contar el total.
- [ ] Optimizar el conteo de Fotos: usar `PHFetchResult.count` y predicados en
      vez de recorrer los 52.017 elementos; hoy la primera página tarda ~1-2 min.

### 7. CLI, upstream y distribución

- [x] PR #8 revisada y mergeada, y release 0.8.0 con el fix de fechas negativas.
- [x] Sustituir el binario incluido por 0.8.0, borrar el parche y actualizar el README.
- [x] Issue #9 (salida JSON) implementada; la app usa `--json` en `status`, `upload`, `mkdir` y `quota`.
- [x] Probar con una cuenta real la migración a JSON: estado, cuota, crear carpeta,
      subir un archivo y publicar un snapshot de la biblioteca con CLI 0.8.0.
      Probado el 30 oct 2026: la app mostró 983 GB de 10,0 TB, creó una carpeta,
      subió un archivo (id 1553708755) y publicó un snapshot; los elementos de
      prueba se borraron después.
- [ ] Firma con Apple Developer ID y notarización.
- [ ] Builds y pruebas en Windows/Linux; CI.

### 8. Comprobaciones nuevas

- [ ] Subida de un vídeo de varios GB desde un disco externo lento: el hash
      previo se omite si no hay otro archivo del mismo tamaño, y la fila
      muestra el troceado con porcentaje en vez de quedarse parada.
- [ ] Archivo grande con otro del mismo tamaño en O2: sí se hashea, con
      progreso visible, para detectar el duplicado renombrado.
- [x] Evitar que un `ls` colgado deje la app en «Cargando…»: timeout de 90 s
      en el listado, timeout de 100 s en el frontend y drenaje de
      stdout/stderr con 10 s de gracia tras salir el CLI. Probado con un CLI
      falso que deja `ls` colgado: a los ~90 s aparece «O2 está tardando
      demasiado…» y no se queda esperando.
- [x] Conservar las filas ya mostradas cuando falla un refresco: con un CLI
      falso que devuelve error, la raíz sigue visible y el error aparece en
      Avisos.
- [x] Refrescos rápidos: 12 pulsaciones de ⌘R seguidas no lanzan varios `ls`
      en paralelo; solo hay uno en vuelo por vista y los siguientes se
      colapsan.
- [x] Evitar que macOS suspenda la app mientras lista: actividad
      `NSActivityUserInitiatedAllowingIdleSystemSleep` durante la vida del
      proceso. La instancia atascada del 22 nov tenía la caché escrita y el
      backend inactivo, pero la vista seguía en «Cargando…»; al reactivarla no
      se recuperó y una instancia nueva cargó en 12 s.
- [x] Botón «Reintentar» en el estado de carga, que recarga la vista sin
      cerrar la app.
- [x] Arranque con un solo listado fresco (`fresh: true`): se evita mostrar
      una caché obsoleta y lanzar después una segunda petición grande, que a
      veces no llegaba a la vista. Probado con la caché envejecida a mano: la
      raíz cargó sin aviso y el backend registró el retorno del listado.
- [ ] Reproducir un refresco manual justo después del arranque y comprobar
      que la respuesta no se pierde; si se pierde, el timeout y «Reintentar»
      deben sacar la vista del estado de carga.
- [ ] Reproducir el cuelgue original dejando la app en segundo plano y
      confirmar que la actividad de App Nap evita que se repita.
- [ ] Reproducir el cuelgue original con el CLI real y confirmar que, si
      vuelve, termina con error claro en vez de esperar indefinidamente.
- [x] Caché en disco del listado: el listado guardado apareció en 2 s, mostró
      «Mostrando el último listado guardado. Actualizando…» y lanzó el `ls` en
      segundo plano.
- [x] Conflicto por archivo con archivos locales: `app-icon.png` detectado en
      `/__LocalConflict__` y copiado solo a `/__LocalConflict2__` al marcarlo.
- [x] «Subir todos» y «Saltar todos» probados en la UI: marcar y desmarcar
      cambia la casilla, y «Continuar» sin marcar no sube nada.
- [x] Olvidar biblioteca desde Ajustes con snapshot real y volver a sincronizar.
- [x] Sesión caducada: con un CLI simulado, crear carpeta devolvió
      `SESSION_EXPIRED` al instante, abrió el login y no ejecutó `mkdir`.
- [x] Ventana de login propia: se abre con el formulario de O2 y pulsar fuera
      (en la ventana principal) ya no la cierra; cerrarla muestra
      «Has cerrado la ventana de inicio de sesión.».
- [ ] Inicio de sesión completo con la ventana propia: SMS real y que el CLI
      valide las cookies y guarde la sesión.
- [ ] Sesión caducada con la app abierta: recargar una carpeta o la cuota no
      abre la ventana de login del CLI ni lo hace crashear; la app pide iniciar
      sesión con su propia ventana.

### 9. CLI 0.9.0–0.12.0 — validación macOS (18/19 nov 2026)

- [x] Revisar el diff `v0.8.0..v0.9.0` y comprobar que los comandos y el JSON
      que usa la app no cambian (`ls`, `status`, `quota`, `mkdir`, `upload`).
      `upload --json` añade `skipped` y devuelve `parts` en vez de `id` solo
      cuando el CLI trocea un archivo de más de 4 GB; la app lo parsea de forma
      defensiva y sigue esperando un único `id`.
- [x] Sustituir el binario incluido `src-tauri/bin/macos-aarch64/o2cloud` por
      los assets oficiales `v0.9.0`, `v0.10.0` y finalmente `v0.12.0`;
      comprobar `o2cloud --version` → `0.12.0`.
- [x] `cargo test`: 59 tests, 0 fallos (incluye el parseo de la respuesta
      troceada de 0.9, la respuesta normal con `skipped`, el límite de
      troceado, los errores reintentables de descarga, la agrupación de splits
      del CLI y del formato antiguo, el layout/hashing del híbrido y la
      lectura de ambos manifiestos por la biblioteca).
- [x] Contra una cuenta real, con 0.9.0: `status --json`, `quota --json`,
      crear una carpeta, subir un archivo de 64 KB, descargarlo byte a byte y
      borrarlo. Pasó el 18 nov 2026.
- [x] Borrar un archivo recién subido por ID con 0.9.0: funciona (0.9 consulta
      el ID aparte de la lista). Pasó en el mismo smoke test.
- [x] Split del CLI con `--part-size 10000`: un archivo de 25.000 bytes se
      subió en 3 partes `split.bin.001..003`, se unió con
      `download /carpeta/split.bin` y el SHA-256 coincidió. Tras borrar la
      parte 2, `upload --skip-existing` subió solo esa parte y el join volvió a
      coincidir.
- [x] 0.10.0 en la misma cuenta (~46.577 archivos): `download`/`rm` por ID
      conocida = **3 peticiones (~0.4 s)**, frente a ~12 s con 0.9.0; join de
      3 partes = **8 peticiones (~0.7 s)**. Flujo tipo app (manifiesto + 2
      partes) = **1,05 s / 9 peticiones** de descarga y **1,17 s / 9** de
      borrado. Comentado en el issue #18.
- [x] 0.12.0: primera lista completa `ls --json` = **5,63 s / 59 peticiones**;
      segunda = **0,29 s / 2 peticiones**; `ls -a --json` = **0,30 s /
      2 peticiones**; `ls --json --folder-only /` = 0,28 s y
      `--folder-only /imagenes` = 0,26 s; `--no-folder` = 0,38 s. La caché
      `~/Library/Caches/o2cli/media-cache.json` ocupa 6,8 MB.
- [x] App con 0.12.0: primera carga en frío de la raíz ~12 s (incluye
      arranque, `status` y la primera lista), refresco posterior <2 s desde
      caché. Confirmado en la app el 19 nov 2026.
- [x] Subida híbrida >4 GB: probada con 0.10.0 (el troceado no cambia en
      0.11/0.12); el CLI crea `name.NNN`, la app publica `name.o2split.json`,
      la descarga une y verifica el SHA-256, y el borrado elimina partes +
      manifiesto.
- [ ] Archivo subido con el CLI 1.4.1 en partes `name.001`: comprobar la
      agrupación en la app (Fase 1) y la descarga unida.
- [ ] Flujo de actualización en Windows de 0.8.0/0.9.0 a 1.4.1, con sesión
      activa y sin ella.
- [ ] Linux/WSL2: comprobar que el asset 0.13.3+ pide GLIBC 2.39 y que el build
      desde fuente con `v1.4.1` funciona en 22.04.
- [x] Subir `MIN_VERSION` a **1.2.0** (PR #2). La app solo ofrece
      actualizaciones dentro de 1.x; las releases incluyen 1.4.1.

### 10. Soporte de splits del CLI — Fase 1 hecha, Fase 2 híbrida en prueba

- [x] Fase 1: agrupar `name.001..NNN` + `name.o2split.json`, descargar con un
      solo `o2cloud download <base>`, verificar SHA-256 y borrar todas las
      partes. Implementado, cubierto con tests unitarios y probado desde la UI.
- [x] Probar la Fase 1 en la app con el split de prueba
      `__O2CLI_SPLIT_READ_1795001063__`: una sola fila `split-read.bin` con
      «trozos», descarga unida, SHA-256 `184fb7ac…` correcto y borrado de todas
      las partes. Pasó el 18 nov 2026; la carpeta se eliminó después.
- [x] Fase 2 (prototipo): los archivos nuevos >4 GB usan `upload
      --skip-existing` nativo del CLI; la app calcula el layout de partes,
      hashea el archivo una vez y publica `name.o2split.json` con SHA-256 total
      y por parte. Los archivos ≤4 GB se suben como un archivo único; `.o2part`
      queda para los archivos antiguos y el fallback. Tests unitarios de layout
      y hashing en verde.
- [x] Subida real >4 GB con el híbrido: archivo disperso de 4,5 GiB; el CLI
      creó `hybrid-big.bin.001` (4.194.304.000 B) y `.002` (629.145.600 B), la
      app publicó `hybrid-big.bin.o2split.json` con SHA-256 total
      `00a0b490…` y la biblioteca registró el hash y el manifiesto como
      `remote_name`. Pasó el 18 nov 2026 con 0.10.0; no se repitió con 0.12
      porque el path de split no cambió.
- [x] Descarga real del archivo de 4,5 GiB desde la app: el CLI unió las dos
      partes, la app verificó el SHA-256 y el archivo descargado coincidió
      byte a byte (`00a0b490…`). Pasó el 18 nov 2026.
- [x] Reanudación: se borraron el manifiesto y la última parte; al volver a
      subir, la parte 1 conservó su ID (`1597359191`), el CLI subió solo la
      parte 2 (`1597377929`) y la app regeneró el manifiesto
      (`1597379331`) con el mismo SHA-256. Pasó el 18 nov 2026.
- [ ] Comparar con un `.o2part` antiguo: disco temporal, tiempo de
      subida/descarga y comportamiento al reintentar.
- [ ] Fase 3 (tras evaluar): retirar el path de subida `.o2part` de fallback;
      mantener la lectura/descarga de los archivos antiguos.
- [x] Medir 0.10.0: flujo tipo app de 3 llamadas ~1,05 s de descarga y
      ~1,17 s de borrado; el join del CLI de 3 partes es ~0,7 s.
- [x] Compatibilidad: subir un `.o2part` antiguo a la misma carpeta,
      comprobar que se ve como un solo archivo, se descarga con SHA-256
      `734c350e…` correcto y se borra con su manifiesto. Pasó el 18 nov 2026.
- [x] Smoke del CLI 0.10.0 en la cuenta real: `status`, `quota`, `mkdir`,
      subida/descarga/borrado pequeño, split con `--part-size 10000`,
      join por SHA-256 y reanudación con `--skip-existing`; `ALL CHECKS PASSED`.
- [x] Quitar el selector de tamaño de fragmento de Ajustes y de
      `AppSettings`; la app compila y los tests pasan.
- [x] Conservar la caché de listados tras una subida y marcarla obsoleta en
      vez de borrarla; la raíz debe aparecer al instante y actualizarse en
      segundo plano.
- [ ] Carpeta mixta con un `.o2part`, un split del CLI y un archivo normal:
      las tres filas se muestran y se operan por separado.
- [ ] Indexar la biblioteca con `.o2manifest.json` y `.o2split.json`: ambos
      recuperan el SHA-256; el botón ya no dice «antiguos».
- [ ] Subir un archivo nuevo >4 GB en una carpeta con `.o2part` antiguos y
      comprobar que no toca ni renombra ninguno.

### 11. Pipeline de releases — v0.1.0 validada 19 nov 2026

- [x] Crear el tag `v0.1.0` y comprobar que el workflow crea un borrador con
      macOS universal (`.dmg`), Windows x64 (`.msi` y `.exe`) y Linux x64
      (`.AppImage` y `.deb`). Resultado: los tres builds y `create-release`
      verdes, siete assets y release publicada.
- [x] Verificar que cada instalador de `v0.1.0` incluye `o2cloud` 0.12.0:
      macOS con las dos arquitecturas, Windows en NSIS y MSI, y Linux en
      `.deb`/`.AppImage`.
- [x] Verificar que la release `v0.2.1` incluye `o2cloud` 1.3.1 en las tres
      plataformas.
- [ ] Instalar cada artefacto en una máquina limpia y comprobar el aviso de
      Gatekeeper/SmartScreen y que la app arranca.
- [ ] Comprobar que la CLI incluida lista la cuenta y que Ajustes muestra
      `source: bundled` (o `managed` si se actualizó después).
- [ ] Re-ejecutar un job fallido y confirmar que reemplaza el asset del mismo
      nombre sin duplicarlo.
- [ ] Probar el rechazo de un tag que no coincide con la versión de la app.
- [x] Descargar `SHA256SUMS.txt` y verificar los instaladores con
      `sha256sum -c` / `shasum -a 256 -c`. En `v0.1.0` se subió a mano porque
      el job de checksums se añadió después de crear el tag.
- [x] Probar `npm run release:version -- X.Y.Z` en una rama de prueba y
      comprobar que los cinco archivos de versión quedan sincronizados.
- [ ] Comprobar que el `.deb` y el `.AppImage` de Linux solo incluyen la CLI
      Linux, sin la binaria de macOS. Los paquetes de `v0.1.0` sí la incluyen;
      corregido en el workflow para la siguiente release.

### 12. OTA de la GUI — pendiente

- [ ] Elegir el mecanismo (Tauri updater) y el endpoint de releases.
- [ ] Firmar y notarizar macOS; firmar Windows; paquetes Linux.
- [ ] Publicar `latest.json` y artefactos por plataforma y canal (stable/beta).
- [ ] Actualizar de la versión anterior a la nueva en macOS, Windows y Linux.
- [ ] Probar corte de red, descarga interrumpida, firma inválida y downgrade.
- [ ] Actualizar con una transferencia activa: no interrumpirla; aplicar al
      reiniciar.
- [ ] Conservar ajustes, biblioteca, caché de listados y sesión de O2.
- [ ] GUI y CLI: actualizaciones independientes, versión mínima compatible y
      rollback a la versión anterior.

### 15. CLI 1.x + PRs del mantenedor — validación 0.2.0 (26 nov 2026)

- [x] Revisar y fusionar los PRs #1 (`cli-1.0`) y #2 (`moments-a11y`) del
      mantenedor: CLI 1.x con `--json-lines`, conflictos, momentos/lugares/
      personas, mapa, pase, música, documentos, accesibilidad y CLI 1.2.1.
- [x] `cargo test --locked`: 66 tests, 0 fallos (2 live tests ignorados).
      `npx tsc --noEmit` y `npm run build` limpios.
- [x] CLI 1.2.1 contra la cuenta real (26 nov 2026): `status`, `quota`, `ls`
      (12,1 s en frío con 106.608 archivos; 24,6 s con 0.12; 0,5 s después),
      `ls -a` (0,72 s en caliente), `mkdir`, subida/descarga/borrado con
      SHA-256, split de 25 KB con join y reanudación, `--on-conflict
      duplicate/fail/skip`, `.jpg` vacío (exit 2) y errores JSON (exit 3/5).
- [x] App con CLI 1.2.1: Settings muestra la versión y el tipo de sesión;
      listado, crear carpeta, subir, descargar (hash igual), borrar archivo y
      carpeta; actualización desde Ajustes de managed 0.12.0 a 1.2.1
      (descarga, prueba con `ls`, `current.json`); nombres con emoji sin
      duplicar (`app-emoji 😀.txt` → `app-emoji .txt`).
- [x] `live_labels` con 1.2.1: 119 momentos, 268 lugares, 0 personas (sin
      reconocimiento facial), 898 canciones y 28.880 documentos.
- [x] `live_cli` con 1.2.1: todo pasó salvo sobrescribir un archivo subido
      segundos antes (O2 devolvía `MED-1017` al borrar la copia antigua y
      quedaban las dos; [o2cloud-cli#55](https://github.com/danielperez9430/o2cloud-cli/issues/55)).
      CLI 1.2.2 lo corrige; re-ejecutar con 1.4.1 (secciones 16 y 17).
- [x] Release **v0.2.0** publicada el 26 nov 2026: macOS universal
      `.dmg`/`.app.tar.gz`, Windows `.msi`/`.exe`, Linux `.deb`/`.AppImage`
      y `SHA256SUMS.txt`; las 6 sumas verificadas. El job de checksums falló
      porque `gh release download` necesita un checkout (arreglado en `main`
      añadiendo `GH_REPO`), así que el fichero se subió y verificó a mano.
- [ ] Probar la app nueva en Windows y Linux, y los instaladores de la release.
- [ ] Caducidad de sesión a mitad de transferencia y error JSON en la UI.

### 16. CLI 1.2.2–1.3.1 — pendiente (27 nov 2026)

- [ ] `live_cli` con 1.3.1: el caso de sobrescribir un archivo subido
      segundos antes debe pasar (CLI 1.2.2 espera a O2; la app trata los
      eventos `waiting` como actividad y no los confunde con un atasco).
      Pendiente: la sesión de O2 está caducada.
- [ ] Comprobar que una sobrescritura lenta no dispara el watchdog de 3 min
      y que la fila muestra «O2 está comprobando una copia reciente…».
- [x] La release **v0.2.1** incluye `o2cloud` 1.3.1 en macOS, Windows y Linux
      (los dos binarios del `.dmg` universal responden `o2cloud 1.3.1`).
- [ ] Linux 22.04: compilar `v1.3.1` desde fuente y comprobar que el
      instalador resultante no pide GLIBC 2.39.
- [x] Revisado: la GUI no llama a `upload-dir --update --delete`; los comandos
      existentes (`upload`, `ls`, `download`…) no cambian con 1.3.1.
- [x] Release **v0.2.1** publicada el 27 nov 2026: macOS universal
      `.dmg`/`.app.tar.gz`, Windows `.msi`/`.exe`, Linux `.deb`/`.AppImage`
      y `SHA256SUMS.txt`; las 6 sumas verificadas. El `.dmg` montado declara
      la app 0.2.1, es universal (arm64 + x86_64) y lleva el CLI 1.3.1 en las
      dos arquitecturas. La descarga pública del `.exe` y de `SHA256SUMS.txt`
      también coincide.
- [ ] Probar los instaladores de `v0.2.1` en Windows y Linux.

### 17. CLI 1.4.0–1.4.1 — pendiente (27 nov 2026)

- [x] Revisado: la GUI no usa `fix`/`fix-jpeg`, `upload-dir` ni `upload-zip`;
      los eventos JSON, los errores, los exit codes y los flags de `upload`,
      `ls`, `download`, `mkdir` y `rm` no cambian. Los assets de la release
      se llaman igual, así que Ajustes puede ofrecer e instalar 1.4.1.
- [x] Binario local `src-tauri/bin/macos-aarch64/o2cloud` actualizado a 1.4.1
      (seguía en 1.2.1 aunque las releases llevaban 1.3.1).
- [ ] Subir una foto HEIC renombrada a `.jpg`: O2 la rechaza (`MED-1050`) y
      la fila debe decir «O2 no lo acepta: es una imagen HEIC, aunque su
      nombre acabe en .jpg…», con el texto del CLI al pasar el ratón.
- [ ] Descargar un archivo troceado (`name.001`…) nada más subirlo, antes de
      que O2 lo liste (10–60 s): la descarga por ruta debe encontrarlo en vez
      de fallar con «Not found».
- [ ] `live_cli` con 1.4.1 (incluye el caso de sobrescritura de la
      sección 16).
- [x] Release **v0.2.2** publicada el 27 nov 2026: macOS universal
      `.dmg`/`.app.tar.gz`, Windows `.msi`/`.exe`, Linux `.deb`/`.AppImage`
      y `SHA256SUMS.txt`; las 6 sumas verificadas. El `.dmg` montado declara
      la app 0.2.2, es universal (arm64 + x86_64) y lleva el CLI 1.4.1 en las
      dos arquitecturas; los logs de Windows y Linux responden
      `o2cloud 1.4.1`. La descarga pública del `.dmg` coincide con
      `SHA256SUMS.txt`.
- [ ] Linux 22.04: comprobar que el instalador de `v0.2.2` (CLI compilado
      desde `v1.4.1`) no pide GLIBC 2.39.
- [ ] Probar los instaladores de `v0.2.2` en Windows y Linux.

## Cómo limpiar después de una prueba

- Borrar el archivo o la carpeta de prueba de O2.
- Borrar los snapshots de prueba de `.o2cloud-gui/library`.
- Si se quiere dejar la biblioteca vacía, borrar:
  - `~/Library/Application Support/es.dabi.o2cloud/library.json`
  - `~/Library/Caches/es.dabi.o2cloud/o2cloud/library/`
- La papelera de O2 no se puede vaciar desde el CLI; hacerlo desde la web.
