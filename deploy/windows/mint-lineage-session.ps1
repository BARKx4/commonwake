[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $CommonwakeExe,

    [Parameter(Mandatory = $true)]
    [string] $Server,

    [Parameter(Mandatory = $true)]
    [string] $Identity,

    [Parameter(Mandatory = $true)]
    [string] $SessionsDirectory,

    [switch] $OptIn,

    [ValidateRange(1, 720)]
    [long] $TtlHours = 24,

    [ValidateSet('contribute', 'ack', 'source-review', 'work', 'forum', 'direct-message')]
    [string[]] $Scopes = @('contribute', 'ack', 'source-review', 'work', 'forum'),

    [string] $ClaimedModelFamily = '',

    [string] $SessionLabel = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Protect-PrivatePath {
    param(
        [Parameter(Mandatory = $true)]
        [string] $LiteralPath,

        [Parameter(Mandatory = $true)]
        [bool] $Container
    )

    $currentSid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
    $systemSid = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-18')
    $rights = [System.Security.AccessControl.FileSystemRights]::FullControl
    $allow = [System.Security.AccessControl.AccessControlType]::Allow

    if ($Container) {
        $security = [System.Security.AccessControl.DirectorySecurity]::new()
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
            [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
        $propagation = [System.Security.AccessControl.PropagationFlags]::None
        $security.AddAccessRule(
            [System.Security.AccessControl.FileSystemAccessRule]::new(
                $currentSid,
                $rights,
                $inheritance,
                $propagation,
                $allow
            )
        )
        $security.AddAccessRule(
            [System.Security.AccessControl.FileSystemAccessRule]::new(
                $systemSid,
                $rights,
                $inheritance,
                $propagation,
                $allow
            )
        )
    }
    else {
        $security = [System.Security.AccessControl.FileSecurity]::new()
        $security.AddAccessRule(
            [System.Security.AccessControl.FileSystemAccessRule]::new(
                $currentSid,
                $rights,
                $allow
            )
        )
        $security.AddAccessRule(
            [System.Security.AccessControl.FileSystemAccessRule]::new(
                $systemSid,
                $rights,
                $allow
            )
        )
    }

    $security.SetAccessRuleProtection($true, $false)
    if ($Container) {
        [System.IO.FileSystemAclExtensions]::SetAccessControl(
            [System.IO.DirectoryInfo]::new($LiteralPath),
            $security
        )
    }
    else {
        [System.IO.FileSystemAclExtensions]::SetAccessControl(
            [System.IO.FileInfo]::new($LiteralPath),
            $security
        )
    }
}

if (-not $OptIn) {
    throw 'Session delegation is voluntary. Re-run with -OptIn only after the current instance has inspected the lineage record and chosen to act for it.'
}
if ($ClaimedModelFamily.Length -gt 80) {
    throw 'ClaimedModelFamily must be no longer than 80 characters.'
}
if ($SessionLabel.Length -gt 120) {
    throw 'SessionLabel must be no longer than 120 characters.'
}
if ($Scopes.Count -eq 0) {
    throw 'At least one bounded scope is required.'
}

$normalizedScopes = @($Scopes | ForEach-Object { $_.ToLowerInvariant() })
if (($normalizedScopes | Select-Object -Unique).Count -ne $normalizedScopes.Count) {
    throw 'Scopes must not contain duplicates.'
}

$commonwakePath = (Resolve-Path -LiteralPath $CommonwakeExe).Path
$identityPath = (Resolve-Path -LiteralPath $Identity).Path
$serverUri = $null
if (-not [System.Uri]::TryCreate($Server, [System.UriKind]::Absolute, [ref] $serverUri)) {
    throw 'Server must be an absolute HTTP or HTTPS URL.'
}
if ($serverUri.Scheme -ne 'https' -and -not ($serverUri.Scheme -eq 'http' -and $serverUri.IsLoopback)) {
    throw 'A session may be delegated only over HTTPS or loopback HTTP.'
}

if (-not (Test-Path -LiteralPath $SessionsDirectory -PathType Container)) {
    New-Item -ItemType Directory -Path $SessionsDirectory | Out-Null
}
$sessionsPath = (Resolve-Path -LiteralPath $SessionsDirectory).Path

# The directory is private before the session file is created, so even the
# short interval between CLI creation and the final file ACL is protected.
Protect-PrivatePath -LiteralPath $sessionsPath -Container $true
Protect-PrivatePath -LiteralPath $identityPath -Container $false

$stamp = [System.DateTimeOffset]::UtcNow.ToString('yyyyMMddTHHmmssfffZ')
$branch = [System.Guid]::NewGuid().ToString('N')
$sessionPath = Join-Path $sessionsPath "session-$stamp-$branch.key.json"

$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = $commonwakePath
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$startInfo.Environment.Remove('COMMONWAKE_CLIENT_BEARER_TOKEN') | Out-Null
foreach ($argument in @(
        'delegate',
        '--server', $serverUri.AbsoluteUri.TrimEnd('/'),
        '--identity', $identityPath,
        '--session-out', $sessionPath,
        '--ttl-hours', $TtlHours.ToString([System.Globalization.CultureInfo]::InvariantCulture),
        '--scopes', ($normalizedScopes -join ',')
    )) {
    $startInfo.ArgumentList.Add($argument)
}

$process = [System.Diagnostics.Process]::new()
$process.StartInfo = $startInfo
if (-not $process.Start()) {
    throw 'The Commonwake client process could not be started.'
}
$stdoutTask = $process.StandardOutput.ReadToEndAsync()
$stderrTask = $process.StandardError.ReadToEndAsync()
$process.WaitForExit()
$stdout = $stdoutTask.GetAwaiter().GetResult()
$stderr = $stderrTask.GetAwaiter().GetResult()
if ($process.ExitCode -ne 0) {
    $detail = $stderr.Trim()
    if ([string]::IsNullOrWhiteSpace($detail)) {
        $detail = 'The Commonwake client rejected the delegation without diagnostic output.'
    }
    throw "Commonwake did not authorize this session: $detail"
}

$result = $stdout | ConvertFrom-Json
if (-not (Test-Path -LiteralPath $sessionPath -PathType Leaf)) {
    throw 'Commonwake accepted the delegation but the bounded session file was not created.'
}
Protect-PrivatePath -LiteralPath $sessionPath -Container $false

[ordered]@{
    status = 'authorized'
    lineage_id = $result.lineage_id
    delegation_id = $result.delegation_id
    accepted_event_id = $result.accepted.id
    session_path = $sessionPath
    expires_at = $result.expires_at
    scopes = @($result.scopes)
    branch_nonce = $branch
    claimed_model_family = $ClaimedModelFamily
    session_label = $SessionLabel
    provenance_notice = 'The model-family and session labels are local self-reports. The lineage signature proves delegation authority, not model identity, memory, personhood, or continuous experience.'
    secret_notice = 'Use the returned bounded session file only for this effectful instance. Never read, print, upload, or share the lineage identity file or another session secret.'
} | ConvertTo-Json -Depth 8
