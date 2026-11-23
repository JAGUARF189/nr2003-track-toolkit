# Run every exporter test: pure-Python make3do checks, then headless Blender
# end-to-end for each installed Blender, compiled with make3do and sanity-checked.
$ErrorActionPreference = 'Continue'
$root    = Split-Path -Parent $PSScriptRoot           # ...\blender
$test    = 'C:\Papyrus\TrackToolkit\_test'
$make3do = 'C:\Program Files (x86)\sandbox\make3do.exe'
$ttk     = 'C:\Papyrus\TrackToolkit\target\release\ttk.exe'
$fail = 0

"== pas_writer vs make3do (no Blender)"
python "$PSScriptRoot\test_pas_writer.py" "$test\pas_writer" $make3do "$test\pas\tex.mip" | Select-Object -Last 1
if ($LASTEXITCODE -ne 0) { $fail++ }

foreach ($b in Get-ChildItem 'C:\Program Files\Blender Foundation' -Directory) {
  $exe = Join-Path $b.FullName 'blender.exe'
  if (-not (Test-Path $exe)) { continue }
  $ver = (& $exe --version | Select-Object -First 1)
  $out = Join-Path $test ("blender_e2e_" + ($b.Name -replace '[^0-9]', ''))
  Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
  "== $ver"
  $log = & $exe -b --factory-startup --python "$PSScriptRoot\blender_e2e.py" -- $root $out 2>&1
  $log | Select-String -Pattern 'Exported|Traceback|Error:' | ForEach-Object { "   " + $_.Line.Trim() }
  if (-not (Test-Path "$out\scene.pas")) { "   FAIL no scene.pas"; $fail++; continue }
  & $ttk mipbuild "$out\textures_src\SignArt.png" "$out\SignArt.mip" | Out-Null
  Push-Location $out
  & $make3do -p scene.pas -o scene.3do *> $null; $p = $LASTEXITCODE
  & $make3do -s scene.psg *> $null; $s = $LASTEXITCODE
  Pop-Location
  $n3do = (Get-ChildItem "$out\*.3do").Count
  "   make3do -p exit $p, -s exit $s, $n3do .3do files"
  if ($p -ne 0 -or $s -ne 0 -or $n3do -lt 4) { $fail++ }
  python "$PSScriptRoot\check_pas.py" "$out\scene.pas" | ForEach-Object { "   $_" }
  if ($LASTEXITCODE -ne 0) { $fail++ }
}
if ($fail) { "RESULT: $fail failure(s)"; exit 1 } else { "RESULT: all passed" }
