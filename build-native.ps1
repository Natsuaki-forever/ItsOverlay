# Portable build entry: Slint (-SlintEditor / -Trial) is the active editor direction;
# omitting -SlintEditor builds the frozen/fallback Win32 editor.
# Builds the native C++ OverlayVision.exe worker by default (pass -VisionDeps for
# the legacy Python/OpenCV worker, or -NoVision to omit the visual worker).
# The retired Qt editor build path is archived on branch `codex/archive-qt-editor`.
param(
    [string]$Output = 'dist\ChatGPTOverlay-Current',
    [string]$Work = 'build\current',
    [string]$Python = 'python',
    [string]$VisionDeps,
    [ValidatePattern('^\d{8}-r[1-9]\d*$')][string]$ReleaseId,
    [switch]$Zip,
    [switch]$VerifyEditor,
    [switch]$ReplaceExisting,
    [switch]$SlintEditor,
    [switch]$Trial,
    [switch]$PublishStaged,
    [switch]$StageOnly,
    [switch]$NoVision
)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
. (Join-Path $PSScriptRoot 'tools\publish-native.ps1')
if ($PublishStaged -and (-not $Trial -or $StageOnly)) {
    throw 'PublishStaged installs a previously verified trial.'
}
if ($VisionDeps -and $NoVision) { throw 'Choose either -VisionDeps or -NoVision.' }
if ($Trial) {
    if ($PSBoundParameters.ContainsKey('Output')) { throw 'Trial uses the fixed trial delivery directory.' }
    if ($Zip) { throw 'Trial uses Slint and is published as a local directory.' }
    $Output = 'dist\ChatGPTOverlay-Trial'
    if (-not $PSBoundParameters.ContainsKey('Work')) { $Work = 'build\trial-work' }
    $SlintEditor = $true
    $VerifyEditor = $true
}
$issueDirectory = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'ChatGPTOverlay\diagnostics'
$published = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot $Output))
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot $Work))
$distRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'dist')) + '\'
$buildRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'build')) + '\'
if (-not $published.StartsWith($distRoot, [StringComparison]::OrdinalIgnoreCase) -or
    -not $workspace.StartsWith($buildRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Output and Work must be child directories of this project dist and build.'
}
Assert-OverlayDeliveryPath $workspace $PSScriptRoot
if ($StageOnly -and $Zip) { throw 'StageOnly builds files for verification without creating a release archive.' }
if (-not $StageOnly) { Restore-InterruptedOverlayDelivery $published $PSScriptRoot $issueDirectory }
$previousRelease = ''
$previousManifest = Join-Path $published 'build.json'
if (Test-Path -LiteralPath $previousManifest) {
    $previousRelease = ([IO.File]::ReadAllText($previousManifest) | ConvertFrom-Json).release_id
}
if (-not $ReleaseId) {
    $date = [TimeZoneInfo]::ConvertTimeBySystemTimeZoneId([DateTime]::UtcNow, 'China Standard Time').ToString('yyyyMMdd')
    $last = 0
    if ($previousRelease -match ('^' + $date + '-r([1-9]\d*)$')) { $last = [int]$Matches[1] }
    foreach ($archive in Get-ChildItem -LiteralPath (Split-Path -Parent $published) -Filter '*.zip' -ErrorAction SilentlyContinue) {
        if ($archive.Name -match ('^ChatGPTOverlay-当前版-' + $date + '-r([1-9]\d*)\.zip$')) { $last = [Math]::Max($last, [int]$Matches[1]) }
    }
    $ReleaseId = $date + '-r' + ($last + 1)
}
$archivePath = Join-Path (Split-Path -Parent $published) ('ChatGPTOverlay-当前版-' + $ReleaseId + '.zip')
if ($Zip -and (Test-Path -LiteralPath $archivePath)) { throw 'This release archive already exists; use a new release ID.' }
function Assert-Replaceable {
    if (-not (Test-Path -LiteralPath $published)) { return }
    if (-not $ReplaceExisting) { throw 'Use -ReplaceExisting to update the fixed delivery directory.' }
    if (-not (Test-Path -LiteralPath (Join-Path $published 'build.json'))) { throw 'Existing output is not an identified build.' }
    $allowed = @('ItsOverlay.exe','ChatGPTOverlay.exe','editor','vision','licenses','README.md','VERSION.txt','build.json','VERIFICATION.json','ACCEPTANCE_CHECKLIST.md','SHA256SUMS.txt')
    foreach ($entry in Get-ChildItem -LiteralPath $published -Force) {
        if ($entry.Name -notin $allowed -or ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Existing output contains unexpected files; preserve and review it before replacement.'
        }
    }
    foreach ($process in Get-Process -Name ItsOverlay,ChatGPTOverlay,OverlayEditor,OverlayVision -ErrorAction SilentlyContinue) {
        if (-not $process.Path) { throw 'Cannot check a running product process; close the app before replacement.' }
        if ($process.Path.StartsWith($published + '\', [StringComparison]::OrdinalIgnoreCase)) {
            Write-OverlayUpdateIssue $issueDirectory 'update_running'
            throw 'The current delivery is running. Save and exit it before replacement.'
        }
    }
}
if (-not $StageOnly) { Assert-Replaceable }
$destination = Join-Path $workspace 'delivery'
Assert-OverlayDeliveryPath $destination $PSScriptRoot
if ($PublishStaged) {
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $destination 'build.json') | ConvertFrom-Json
    $results = Get-Content -Raw -LiteralPath (Join-Path $destination 'VERIFICATION.json') | ConvertFrom-Json
    if ($manifest.role -ne 'trial' -or !$results.saved_unicode_text -or
        !$results.visual_geometry_tests_passed -or !$results.worker_process_startup_verified) {
        throw 'The staged trial must pass the editor, visual geometry and worker startup tests.'
    }
    Switch-OverlayDelivery $destination $published $PSScriptRoot -IssueDirectory $issueDirectory
    Write-Host "Installed verified trial: $published"
    return
}
if (Test-Path -LiteralPath $destination) {
    if ((Get-Item -LiteralPath $destination).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Build staging must not be a link.' }
    Remove-Item -LiteralPath $destination -Recurse -Force
}
$oldVisionDeps = $env:OVERLAY_VISION_BUILD_DEPS
$oldPythonPath = $env:PYTHONPATH
$preparing = $true
try {
    & (Join-Path $PSScriptRoot 'native-engine\build.ps1') -Output (Join-Path $workspace 'engine')
    if ($SlintEditor) {
        & (Join-Path $PSScriptRoot 'native-engine\build.ps1') -SlintEditor -Output (Join-Path $workspace 'slint-editor')
    } else {
        & (Join-Path $PSScriptRoot 'native-engine\build.ps1') -Editor -Output (Join-Path $workspace 'native-editor')
    }
    if ($VisionDeps) {
        $env:OVERLAY_VISION_BUILD_DEPS = (Resolve-Path -LiteralPath $VisionDeps).Path
        $env:PYTHONPATH = $env:OVERLAY_VISION_BUILD_DEPS + [IO.Path]::PathSeparator + $oldPythonPath
        & $Python -m PyInstaller --noconfirm --clean --distpath (Join-Path $workspace 'bundle') --workpath (Join-Path $workspace 'vision-pyinstaller') VisualWorker.spec
        if ($LASTEXITCODE -ne 0) { throw 'Visual worker build failed' }
        $env:PYTHONPATH = $oldPythonPath
    } elseif (-not $NoVision) {
        & (Join-Path $PSScriptRoot 'native-engine\build.ps1') -VisionWorker -Output (Join-Path $workspace 'native-vision')
    }
    New-Item -ItemType Directory -Path $destination | Out-Null
    Copy-Item -LiteralPath (Join-Path $workspace 'engine\OverlayNative.exe') -Destination (Join-Path $destination 'ItsOverlay.exe')
    $editorDestination = Join-Path $destination 'editor'
    New-Item -ItemType Directory -Path $editorDestination | Out-Null
    $editorFile = if ($SlintEditor) { 'slint-editor\OverlayEditor.exe' } else { 'native-editor\OverlayNativeEditorPrototype.exe' }
    Copy-Item -LiteralPath (Join-Path $workspace $editorFile) -Destination (Join-Path $editorDestination 'OverlayEditor.exe')
    if ($VisionDeps) {
        Copy-Item -LiteralPath (Join-Path $workspace 'bundle\OverlayVision') -Destination (Join-Path $destination 'vision') -Recurse
    } elseif (-not $NoVision) {
        $visionDestination = Join-Path $destination 'vision'
        New-Item -ItemType Directory -Path $visionDestination | Out-Null
        Copy-Item -LiteralPath (Join-Path $workspace 'native-vision\OverlayVision.exe') -Destination (Join-Path $visionDestination 'OverlayVision.exe')
    }
    Copy-Item -LiteralPath (Join-Path $workspace 'engine\licenses') -Destination $destination -Recurse
    if ($VisionDeps) {
        $noticeArguments = @('--destination', (Join-Path $destination 'licenses'), '--native-editor', '--vision-deps', $env:OVERLAY_VISION_BUILD_DEPS)
        & $Python (Join-Path $PSScriptRoot 'tools\collect_runtime_notices.py') @noticeArguments
        if ($LASTEXITCODE -ne 0) { throw 'Runtime notice collection failed' }
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'NATIVE_USAGE.md') -Destination (Join-Path $destination 'README.md')
    $visionIncluded = [bool]$VisionDeps -or (-not $NoVision)
    $manifest = [ordered]@{
        release_id = $ReleaseId
        role = if ($Trial) { 'trial' } else { 'current' }
        built_utc = [DateTime]::UtcNow.ToString('o')
        source_commit = (& git rev-parse HEAD)
        source_worktree_dirty = [bool](& git status --porcelain --untracked-files=no)
        architecture = if ($SlintEditor) { 'native-engine-with-on-demand-slint-editor' }
            else { 'native-engine-with-on-demand-win32-editor' }
        engine_file = 'ItsOverlay.exe'
        engine_sha256 = (Get-FileHash -LiteralPath (Join-Path $destination 'ItsOverlay.exe') -Algorithm SHA256).Hash
        editor_sha256 = (Get-FileHash -LiteralPath (Join-Path $destination 'editor\OverlayEditor.exe') -Algorithm SHA256).Hash
        qualification = 'candidate; see docs/history/TEST_REPORT.md in the source repository for earlier verification'
        optional_visual_worker = $visionIncluded
        visual_worker = if ($VisionDeps) { 'python-opencv' } elseif (-not $NoVision) { 'native-cpp' } else { 'none' }
    }
    if ($visionIncluded) { $manifest['vision_sha256'] = (Get-FileHash -LiteralPath (Join-Path $destination 'vision\OverlayVision.exe') -Algorithm SHA256).Hash }
    $manifest | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $destination 'build.json') -Encoding UTF8
    $channel = if ($Trial) { '试用版' } else { '当前版' }
    @("ItsOverlay - 就是遮 | $channel | $ReleaseId", "源码提交: $($manifest.source_commit)", "构建时间 UTC: $($manifest.built_utc)", '验证范围见 VERIFICATION.json；日常源码修改通过 Git 保存。') |
        Set-Content -LiteralPath (Join-Path $destination 'VERSION.txt') -Encoding UTF8
    $preparing = $false
    # Publish only after every build step succeeds; never merge old runtime DLLs.
    if ($VerifyEditor) {
        $verification = Join-Path $workspace 'verify-editor'
        Assert-OverlayDeliveryPath $verification $PSScriptRoot
        if (Test-Path -LiteralPath $verification) {
            Remove-Item -LiteralPath $verification -Recurse -Force
        }
        $test = if ($SlintEditor) { 'experiments\slint-panel\smoke.py' } else { 'tests\packaged_editor_entry.py' }
        $exe = Join-Path $destination 'ItsOverlay.exe'
        if ($SlintEditor) { & $Python $test $exe $verification }
        else { & $Python $test --exe $exe --output $verification }
        if ($LASTEXITCODE -ne 0) {
            Write-OverlayUpdateIssue $issueDirectory 'update_verification_failed'
            throw 'Staged editor verification failed; current delivery was preserved.'
        }
        $resultsPath = Join-Path $destination 'VERIFICATION.json'
        if ($Trial) {
            $geometryVerification = Join-Path $workspace 'verify-visual-geometry'
            $workerVerification = Join-Path $workspace 'verify-visual-worker'
            foreach ($run in @($geometryVerification, $workerVerification)) {
                Assert-OverlayDeliveryPath $run $PSScriptRoot
                if (Test-Path -LiteralPath $run) { Remove-Item -LiteralPath $run -Recurse -Force }
            }
            $argsForGeometry = @('--visual-tests', ('"' + $geometryVerification + '"'))
            $geometryTest = Start-Process -FilePath $exe -ArgumentList $argsForGeometry -WindowStyle Hidden -PassThru -Wait
            if ($geometryTest.ExitCode -ne 0) {
                Write-OverlayUpdateIssue $issueDirectory 'update_verification_failed'
                throw 'Staged visual geometry tests failed; installed trial was preserved.'
            }
            & $Python 'tests\visual_package_worker.py' --exe (Join-Path $destination 'vision\OverlayVision.exe') --output $workerVerification
            if ($LASTEXITCODE -ne 0) {
                Write-OverlayUpdateIssue $issueDirectory 'update_verification_failed'
                throw 'Staged visual worker startup tests failed; installed trial was preserved.'
            }
            $parsed = Get-Content -Raw -LiteralPath (Join-Path $verification 'result.json') | ConvertFrom-Json
            $results = [ordered]@{}
            foreach ($prop in $parsed.PSObject.Properties) { $results[$prop.Name] = $prop.Value }
            $results['visual_geometry_tests_passed'] = $true
            $results['worker_process_startup_verified'] = $true
            $results['real_chatgpt_verified'] = $false
            $results | ConvertTo-Json | Set-Content -LiteralPath $resultsPath -Encoding UTF8
        } else {
            Copy-Item -LiteralPath (Join-Path $verification 'result.json') -Destination $resultsPath
        }
        Write-Host "Verified staged editor: $verification"
    }
    $files = Get-ChildItem -LiteralPath $destination -File -Recurse | Sort-Object FullName
    $files | ForEach-Object {
        $relative = $_.FullName.Substring($destination.Length + 1).Replace('\', '/')
        (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + $relative
    } | Set-Content -LiteralPath (Join-Path $destination 'SHA256SUMS.txt') -Encoding ASCII
    if ($StageOnly) { Write-Host "Staged $destination"; return }
    Assert-Replaceable
    New-Item -ItemType Directory -Path (Split-Path -Parent $published) -Force | Out-Null
    Switch-OverlayDelivery $destination $published $PSScriptRoot -IssueDirectory $issueDirectory
    if ($Zip) {
        try { Compress-Archive -LiteralPath $published -DestinationPath $archivePath } catch {
            Write-OverlayUpdateIssue $issueDirectory 'update_archive_failed'
            throw
        }
        if ($previousRelease -match '^\d{8}-r[1-9]\d*$' -and $previousRelease -ne $ReleaseId) {
            $previousArchive = Join-Path (Split-Path -Parent $published) ('ChatGPTOverlay-当前版-' + $previousRelease + '.zip')
            if (Test-Path -LiteralPath $previousArchive) { Remove-Item -LiteralPath $previousArchive }
        }
    }
    Write-Host "Built $published"
} catch {
    if ($preparing) { Write-OverlayUpdateIssue $issueDirectory 'update_prepare_failed' }
    throw
} finally {
    $env:OVERLAY_VISION_BUILD_DEPS = $oldVisionDeps; $env:PYTHONPATH = $oldPythonPath
}
