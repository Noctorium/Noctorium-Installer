# What install.ps1 asks the installer for, tried without a network: its Resolve-Product, taken out of the
# script as it is, against the checksums of a release from before Noctorium Stats -- 0.12.2, which is what
# "latest" is the day the script goes live -- and of one with it.
#
#   powershell -NoProfile -File scripts/tests/products.ps1
#
# Each line is what was given, and then the --product the installer would be run with, whether Stats was
# left out, and whether that left nothing to install -- or "unchanged" when the options go on as they
# were. Exits 1 if any is not what it should be. Plain ASCII, like the script.
param([string]$Script = 'scripts/install.ps1')
$ErrorActionPreference = 'Stop'

$tree = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path $Script), [ref]$null, [ref]$null)
$function = $tree.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Resolve-Product'
    }, $true)
if (-not $function) { throw "$Script has no Resolve-Product" }
. ([scriptblock]::Create($function.Extent.Text))

$hash = '0' * 64
$before = (@('Noctorium-0.12.2-windows-x64.msi', 'noctorium-cli-0.12.2-windows-x64.zip', 'Noctorium-0.12.2.apk',
        'noctorium-installer-cli-windows-x64.exe') | ForEach-Object { "$hash  $_" }) -join "`n"
# With Stats, and the Windows line endings a checksum file may have come with.
$with = $before + "`r`n$hash  noctorium-stats-0.13.0-windows-x64.zip`r`n$hash  Noctorium-Stats-0.13.0.apk`r`n"
# 0.21.0: Stats and Noctorium Rust.
$rust = $with + "$hash  noctorium-rust-0.21.0-windows-x64.zip`n$hash  noctorium-rust-0.21.0-linux-x64.tar.gz`n"
# 0.27.0: Noctorium Song Find as well.
$songFind = $rust + "$hash  noctorium-song-find-0.27.0-windows-x64.zip`n$hash  Noctorium-Song-Find-0.27.0.apk`n"
# Stats for Linux and the phone, and nothing for Windows.
$notForWindows = $before + "`n$hash  noctorium-stats-0.13.0-linux-x64.tar.gz`n$hash  *Noctorium-Stats-0.13.0.apk`n"

$script:wrong = 0
function Test-Choice([string]$expected, [string]$listing, [string[]]$given) {
    $choice = Resolve-Product $given $listing
    $got = if ($null -eq $choice) { 'unchanged' } else { "$($choice.Product)|$($choice.LeftOut)|$($choice.Nothing)" }
    $said = "$(if ($env:NOCTORIUM_PRODUCT) { "NOCTORIUM_PRODUCT=$env:NOCTORIUM_PRODUCT " })$($given -join ' ')"
    if ($got -ceq $expected) {
        Write-Host "ok     $said  ->  $got"
    } else {
        Write-Host "WRONG  $said  ->  $got, not $expected"
        $script:wrong++
    }
}

$env:NOCTORIUM_PRODUCT = $null
# A release from before Stats: left out, said so, and the rest asked for in words its installer knows.
Test-Choice 'both|True|False' $before @('--dry-run', '--product', 'all', '--yes')
Test-Choice '|True|True' $before @('--product', 'stats')
Test-Choice 'desktop|True|False' $before @('--product=desktop,stats')
# What every installer has always taken goes through as it was.
Test-Choice 'both|False|False' $before @('--product', 'both')
Test-Choice 'cli|False|False' $before @('--product', 'CLI')
# Left to the installer: nothing asked, something it will refuse, or something that installs nothing.
Test-Choice 'unchanged' $before @('--yes')
Test-Choice 'unchanged' $before @('--product', 'nonsense')
Test-Choice 'unchanged' $before @('--product', 'stats', '--help')
Test-Choice 'unchanged' $before @('--product')
Test-Choice 'unchanged' $before @('--product', '--yes')
# A release with Stats, for Windows or not.
Test-Choice 'cli,stats|False|False' $with @('--product', 'cli+stats')
Test-Choice 'desktop,cli,stats|False|False' $with @('--product', 'all')
Test-Choice 'stats|False|False' $with @('--product=stats')
Test-Choice '|True|True' $notForWindows @('--product', 'stats')
# Noctorium Rust: alone, beside the rest, and from a release before it, where it is left out.
Test-Choice 'rust|False|False' $rust @('--product', 'rust')
Test-Choice 'desktop,cli,stats,rust|False|False' $rust @('--product', 'all')
Test-Choice 'desktop,rust|False|False' $rust @('--product', 'Rust+desktop')
Test-Choice '|False|True' $with @('--product', 'rust')
Test-Choice 'desktop,cli,stats|False|False' $with @('--product', 'all')
if ((Resolve-Product @('--product', 'rust') $with).RustLeftOut) {
    Write-Host 'ok     --product rust on 0.13.0 says Noctorium Rust is left out'
} else {
    Write-Host 'WRONG  --product rust on 0.13.0 does not say Noctorium Rust is left out'
    $script:wrong++
}
# Noctorium Song Find, the same way.
Test-Choice 'song-find|False|False' $songFind @('--product', 'song-find')
Test-Choice 'desktop,cli,stats,rust,song-find|False|False' $songFind @('--product', 'all')
Test-Choice 'desktop,song-find|False|False' $songFind @('--product', 'Song-Find+desktop')
Test-Choice '|False|True' $rust @('--product', 'song-find')
Test-Choice 'desktop,cli,stats,rust|False|False' $rust @('--product', 'all')
if ((Resolve-Product @('--product', 'song-find') $rust).SongFindLeftOut) {
    Write-Host 'ok     --product song-find on 0.21.0 says Noctorium Song Find is left out'
} else {
    Write-Host 'WRONG  --product song-find on 0.21.0 does not say Noctorium Song Find is left out'
    $script:wrong++
}
# NOCTORIUM_PRODUCT, which --product wins over.
$env:NOCTORIUM_PRODUCT = 'stats'
Test-Choice '|True|True' $before @('--yes')
Test-Choice 'cli|False|False' $before @('--product', 'cli')
$env:NOCTORIUM_PRODUCT = 'desktop, stats'
Test-Choice 'desktop,stats|False|False' $with @()
Test-Choice 'desktop|True|False' $before @()
$env:NOCTORIUM_PRODUCT = $null

Write-Host "$script:wrong wrong"
if ($script:wrong) { exit 1 }
