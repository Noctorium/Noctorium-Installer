# Installs Noctorium on Windows, from PowerShell:
#
#   irm https://noctorium.vercel.app/install | iex
#
# iex has no way to pass options on, so with options it is run as a script block instead:
#
#   & ([scriptblock]::Create((irm https://noctorium.vercel.app/install))) --product cli --yes
#
# It fetches the terminal installer, noctorium-installer-cli-windows-x64.exe, from the latest release,
# checks it against the SHA256SUMS.txt published beside it, and runs it with those options. The installer
# then says what it found and asks before it changes anything; --help lists what else it takes.
#
# It offers Noctorium, the Noctorium CLI and Noctorium Stats, which shows what you have listened to:
# --product desktop, cli, stats, both (Noctorium and the CLI) or all, or several joined by commas, such as
# desktop,stats. NOCTORIUM_PRODUCT says the same, and is how iex is given it:
#
#   $env:NOCTORIUM_PRODUCT = 'stats'; irm https://noctorium.vercel.app/install | iex
#
# Noctorium Stats is new, and a release from before it has none, nor an installer that knows its name. So
# when Stats is asked for, this looks in SHA256SUMS.txt first: if the release has no Stats for Windows, it
# is left out, said so, and the rest is installed as the installer knows it -- nothing is downloaded for
# Stats, and nothing fails for want of it.
#
# Everything is one script block, run by the last line. iex reads the whole text before it runs any of it,
# so a download cut short is a missing brace and an error, never half an install; and nothing in here is
# left behind in the session afterwards. It never calls exit, which through iex would close the window it
# was typed into. A failure is said in red, and $LASTEXITCODE says how it went: the installer's own exit
# status (0 when it did what was asked, 1 when it did not, 2 when the options were not understood), or 1
# when this could not get as far as running it.
#
# Kept to plain ASCII. Windows PowerShell 5.1 reads a file or a response that does not name its encoding as
# the system's code page, and anything past ASCII would arrive mangled.

& {
    Set-StrictMode -Version 2.0
    $ErrorActionPreference = 'Stop'

    $asset = 'noctorium-installer-cli-windows-x64.exe'
    $sums = 'SHA256SUMS.txt'
    # Copied to plain strings, so the installer gets exactly the text that was typed: PowerShell keeps a
    # note on arguments that looked like parameters, which has no business reaching a program.
    $options = @($args | ForEach-Object { [string]$_ })
    $plain = [bool]$env:NO_COLOR -or ($options -contains '--no-color') -or ($options -contains '--no-colour')

    function Say([string]$text, [ConsoleColor]$colour = [ConsoleColor]::Gray) {
        if ($plain) { Write-Host "  $text" } else { Write-Host "  $text" -ForegroundColor $colour }
    }
    function Fail([string]$text) {
        Write-Host ''
        if ($plain) { Write-Host "  $text" } else { Write-Host "  $text" -ForegroundColor Red }
        $global:LASTEXITCODE = 1
    }

    # Asks for */* as curl does. 5.1 sends no Accept at all, GitHub's answers vary by it, and on the day
    # this was written its edge spent a while handing that variant of one file a cached 500 while curl's
    # came down fine. Tried three times, for the same reason.
    function Fetch([string]$name) {
        for ($attempt = 1; ; $attempt++) {
            try {
                Invoke-WebRequest -Uri "$base/$name" -OutFile (Join-Path $folder $name) -Headers @{ Accept = '*/*' } -UseBasicParsing
                return $true
            } catch {
                if ($attempt -lt 3) { Start-Sleep -Seconds 2; continue }
                Fail "Could not download $base/$name"
                Say $_.Exception.Message DarkGray
                return $false
            }
        }
    }

    # What to ask the installer for, from what was asked -- the last --product in [given], or
    # NOCTORIUM_PRODUCT -- and whether the checksums in [listing] have a Noctorium Stats for Windows. Nothing
    # when the options are to go on unchanged: nothing asked for, which is the installer's own question;
    # something this does not know, which is the installer's to refuse, with its 2; or help, a version or a
    # listing, which install nothing. Otherwise the --product to use, whether Stats was left out, and
    # whether that leaves nothing at all.
    function Resolve-Product([string[]]$given, [string]$listing) {
        $asked = $env:NOCTORIUM_PRODUCT
        for ($i = 0; $i -lt $given.Count; $i++) {
            $word = $given[$i]
            if (@('-h', '--help', '-V', '--about', '--list') -ccontains $word) { return $null }
            if ($word -ceq '--product') {
                # --product with nothing after it is a mistake for the installer to point out.
                if ($i + 1 -ge $given.Count -or $given[$i + 1].StartsWith('-')) { return $null }
                $i++
                $asked = $given[$i]
            } elseif ($word.StartsWith('--product=')) {
                $asked = $word.Substring('--product='.Length)
            }
        }
        if (-not $asked) { return $null }

        $desktop = $false; $cli = $false; $stats = $false
        foreach ($one in @($asked.ToLowerInvariant() -split '[,+\s]+' | Where-Object { $_ })) {
            switch -CaseSensitive ($one) {
                'desktop' { $desktop = $true }
                'cli' { $cli = $true }
                'stats' { $stats = $true }
                'both' { $desktop = $true; $cli = $true }
                'all' { $desktop = $true; $cli = $true; $stats = $true }
                default { return $null }
            }
        }
        $leftOut = $false
        if ($stats -and $listing -notmatch '(?m)^\s*[0-9A-Fa-f]{64}\s+\*?noctorium-stats-\d[^/\r\n]*-windows-x64\.zip\s*$') {
            $stats = $false
            $leftOut = $true
        }
        # Joined with commas, which only an installer that knows Noctorium Stats reads -- and Stats is only
        # still here when the release, and so its installer, has it. Without Stats, the words every
        # installer has always taken.
        $product = if ($stats) {
            (@(if ($desktop) { 'desktop' }) + @(if ($cli) { 'cli' }) + 'stats') -join ','
        } elseif ($desktop -and $cli) { 'both' } elseif ($desktop) { 'desktop' } elseif ($cli) { 'cli' } else { '' }
        [pscustomobject]@{ Product = $product; LeftOut = $leftOut; Nothing = -not $product }
    }

    # PowerShell 7 runs on Linux and macOS too, where the Windows installer is of no use. Both have their
    # own one line, the same one.
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        if (Get-Variable -Name IsMacOS -ValueOnly -ErrorAction SilentlyContinue) {
            Fail 'This is the installer for Windows. On a Mac, in Terminal: curl -fsSL https://noctorium.vercel.app/install | sh'
        } else {
            Fail 'This is the installer for Windows. On Linux, in a terminal: curl -fsSL https://noctorium.vercel.app/install | sh'
        }
        return
    }

    # The machine's own architecture, from the registry: a process's environment says what the process was
    # built for, and x64 PowerShell on an ARM64 PC reports AMD64 there.
    $machine = $null
    try {
        $machine = (Get-ItemProperty -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -Name PROCESSOR_ARCHITECTURE).PROCESSOR_ARCHITECTURE
    } catch {
        $machine = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    }
    if ($machine -eq 'ARM64') {
        Say 'This PC is ARM64. Noctorium is built for x64, which Windows 11 runs on ARM through emulation.' DarkGray
    } elseif ($machine -ne 'AMD64') {
        Fail "Noctorium is built for 64-bit Windows, and this is $machine."
        return
    }

    $repository = if ($env:NOCTORIUM_REPOSITORY) { $env:NOCTORIUM_REPOSITORY } else { 'Noctorium/Noctorium-Installer' }
    if ($repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
        Fail "NOCTORIUM_REPOSITORY should be owner/name, such as Noctorium/Noctorium-Installer, not `"$repository`"."
        return
    }
    # The release's own file links rather than the API, so GITHUB_TOKEN is not needed here; the installer
    # reads it for the questions it asks the API.
    $base = "https://github.com/$repository/releases/latest/download"

    $folder = Join-Path ([IO.Path]::GetTempPath()) ('noctorium-install-' + [Guid]::NewGuid().ToString('N'))
    $progress = $ProgressPreference
    # Windows PowerShell 5.1 may still offer only TLS 1.0 and 1.1, which GitHub refused years ago. The
    # setting is the whole session's, so it is put back afterwards. PowerShell 7 does not use it.
    $protocols = $null
    if ($PSVersionTable.PSVersion.Major -lt 6) { $protocols = [Net.ServicePointManager]::SecurityProtocol }
    try {
        if ($null -ne $protocols) {
            [Net.ServicePointManager]::SecurityProtocol = $protocols -bor [Net.SecurityProtocolType]::Tls12
        }
        # 5.1 redraws its progress bar for every block it receives, which makes a download crawl.
        $ProgressPreference = 'SilentlyContinue'

        Write-Host ''
        New-Item -ItemType Directory -Path $folder | Out-Null
        $installer = Join-Path $folder $asset
        # The checksums first: they say whether the release has Noctorium Stats, which decides what the
        # installer is asked for -- and whether there is anything to fetch it for at all.
        if (-not (Fetch $sums)) { return }
        $choice = Resolve-Product $options ([IO.File]::ReadAllText((Join-Path $folder $sums)))
        if ($null -ne $choice) {
            if ($choice.LeftOut) {
                Say 'Noctorium Stats is not in the latest release yet, so it is left out, and nothing is downloaded for it.' DarkGray
            }
            if ($choice.Nothing) {
                Say 'Nothing else was asked for, so nothing has been installed.' DarkGray
                $global:LASTEXITCODE = 0
                return
            }
            # The options again, with --product said once, as worked out: whatever --product said before
            # goes, and NOCTORIUM_PRODUCT is passed on as an option, which an installer from before it reads
            # where it would not read the variable.
            $kept = New-Object System.Collections.Generic.List[string]
            for ($i = 0; $i -lt $options.Count; $i++) {
                if ($options[$i] -ceq '--product') { $i++; continue }
                if ($options[$i].StartsWith('--product=')) { continue }
                $kept.Add($options[$i])
            }
            $kept.Add('--product')
            $kept.Add($choice.Product)
            $options = $kept.ToArray()
        }

        Say "Fetching the Noctorium installer from $repository, the latest release..." DarkGray
        if (-not (Fetch $asset)) { return }
        $ProgressPreference = $progress

        # Lines of "<sha256>  <name>", as sha256sum writes them; a star before the name means binary mode.
        $expected = $null
        foreach ($line in [IO.File]::ReadAllLines((Join-Path $folder $sums))) {
            if ($line -match '^\s*([0-9A-Fa-f]{64})\s+\*?(.+?)\s*$' -and $Matches[2] -ceq $asset) {
                $expected = $Matches[1]
                break
            }
        }
        if (-not $expected) {
            Fail "$sums in that release does not list $asset, so it cannot be checked, and has not been run."
            return
        }
        $actual = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
        if (-not [string]::Equals($actual, $expected, [StringComparison]::OrdinalIgnoreCase)) {
            Fail "$asset does not match $sums, so it has been deleted rather than run."
            Say "expected $($expected.ToLowerInvariant())" DarkGray
            Say "received $($actual.ToLowerInvariant())" DarkGray
            return
        }
        Say "Checked against ${sums}: it matches." Green

        # Run in this console, so it can ask its questions here, and waited for. Whatever it writes to
        # standard error is its own message, not a failure of this script, so it is left alone.
        $ErrorActionPreference = 'Continue'
        try {
            & $installer @options
        } catch {
            Fail "The installer could not be started: $($_.Exception.Message)"
            return
        }
        $global:LASTEXITCODE = $LASTEXITCODE
    } catch {
        Fail "Something went wrong before the installer could run: $($_.Exception.Message)"
    } finally {
        $ProgressPreference = $progress
        if ($null -ne $protocols) { [Net.ServicePointManager]::SecurityProtocol = $protocols }
        Remove-Item -LiteralPath $folder -Recurse -Force -ErrorAction SilentlyContinue
    }
} @args
