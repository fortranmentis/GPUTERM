# GpuTerm status line for native Windows Claude Code.
#
# Claude Code runs Windows status lines through Git Bash when it is installed,
# or PowerShell when it is not. GpuTerm invokes this script explicitly through
# powershell.exe so the same settings command works in both environments.
# Only the whitelisted usage fields below are written to the local snapshot.

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)

function Get-Field($Object, [string]$Name) {
    if ($null -eq $Object) {
        return $null
    }
    $Property = $Object.PSObject.Properties[$Name]
    if ($null -eq $Property) {
        return $null
    }
    return $Property.Value
}

function Add-Field($Target, [string]$Name, $Value) {
    if ($null -ne $Value) {
        $Target[$Name] = $Value
    }
}

function New-PickedObject($Source, [string[]]$Names) {
    $Picked = [ordered]@{}
    foreach ($Name in $Names) {
        Add-Field $Picked $Name (Get-Field $Source $Name)
    }
    if ($Picked.Count -eq 0) {
        return $null
    }
    return $Picked
}

function Get-RemainingPercent($Limits, [string]$Window) {
    $WindowValue = Get-Field $Limits $Window
    $Used = Get-Field $WindowValue "used_percentage"
    if ($null -eq $Used) {
        return $null
    }
    try {
        return [Math]::Max(0.0, [Math]::Min(100.0, 100.0 - [double]$Used))
    }
    catch {
        return $null
    }
}

try {
    $InputJson = [Console]::In.ReadToEnd()
    $Payload = $InputJson | ConvertFrom-Json
    if ($null -eq $Payload) {
        Write-Output ""
        exit 0
    }

    $CapturedAt = [int64](([DateTime]::UtcNow - [DateTime]"1970-01-01").TotalSeconds)
    $Snapshot = [ordered]@{ captured_at = $CapturedAt }

    $SessionId = Get-Field $Payload "session_id"
    if (-not [string]::IsNullOrWhiteSpace([string]$SessionId)) {
        $Snapshot["session_id"] = [string]$SessionId
    }

    $Workspace = Get-Field $Payload "workspace"
    $Cwd = Get-Field $Payload "cwd"
    if ([string]::IsNullOrWhiteSpace([string]$Cwd)) {
        $Cwd = Get-Field $Workspace "current_dir"
    }
    if (-not [string]::IsNullOrWhiteSpace([string]$Cwd)) {
        $Snapshot["cwd"] = [string]$Cwd
    }

    $Model = Get-Field $Payload "model"
    $PickedModel = New-PickedObject $Model @("display_name", "id")
    if ($null -ne $PickedModel) {
        $Snapshot["model"] = $PickedModel
    }

    $Context = Get-Field $Payload "context_window"
    $PickedContext = New-PickedObject $Context @(
        "total_input_tokens",
        "context_window_size",
        "used_percentage",
        "remaining_percentage"
    )
    $CurrentUsage = New-PickedObject (Get-Field $Context "current_usage") @(
        "input_tokens",
        "output_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens"
    )
    if ($null -ne $CurrentUsage) {
        if ($null -eq $PickedContext) {
            $PickedContext = [ordered]@{}
        }
        $PickedContext["current_usage"] = $CurrentUsage
    }
    if ($null -ne $PickedContext) {
        $Snapshot["context_window"] = $PickedContext
    }

    $Cost = Get-Field $Payload "cost"
    $PickedCost = New-PickedObject $Cost @("total_cost_usd", "total_duration_ms")
    if ($null -ne $PickedCost) {
        $Snapshot["cost"] = $PickedCost
    }

    $Limits = Get-Field $Payload "rate_limits"
    $PickedLimits = [ordered]@{}
    foreach ($Window in @("five_hour", "seven_day")) {
        $PickedWindow = New-PickedObject (Get-Field $Limits $Window) @(
            "used_percentage",
            "resets_at"
        )
        if ($null -ne $PickedWindow) {
            $PickedLimits[$Window] = $PickedWindow
        }
    }
    if ($PickedLimits.Count -gt 0) {
        $Snapshot["rate_limits"] = $PickedLimits
    }

    if (-not [string]::IsNullOrWhiteSpace([string]$SessionId)) {
        $UserHome = [Environment]::GetFolderPath("UserProfile")
        if ([string]::IsNullOrWhiteSpace($UserHome)) {
            $UserHome = $HOME
        }
        $Directory = Join-Path $UserHome ".cache\gputerm\agent-status\claude"
        New-Item -ItemType Directory -Force -Path $Directory | Out-Null
        $Target = Join-Path $Directory ("{0}.json" -f $SessionId)
        $Temporary = $Target + ".tmp"
        $Encoded = $Snapshot | ConvertTo-Json -Compress -Depth 8
        $Utf8 = New-Object System.Text.UTF8Encoding($false)
        [IO.File]::WriteAllText($Temporary, $Encoded, $Utf8)
        Move-Item -LiteralPath $Temporary -Destination $Target -Force

        $Cutoff = (Get-Date).AddDays(-7)
        Get-ChildItem -LiteralPath $Directory -File -Filter "*.json" |
            Where-Object { $_.LastWriteTime -lt $Cutoff } |
            Remove-Item -Force
    }

    $Parts = New-Object System.Collections.Generic.List[string]
    $ModelName = Get-Field $Model "display_name"
    if ([string]::IsNullOrWhiteSpace([string]$ModelName)) {
        $ModelName = Get-Field $Model "id"
    }
    if (-not [string]::IsNullOrWhiteSpace([string]$ModelName)) {
        $Parts.Add([string]$ModelName)
    }

    $ContextUsed = Get-Field $Context "used_percentage"
    if ($null -ne $ContextUsed) {
        $Parts.Add(("ctx {0:0}%" -f [double]$ContextUsed))
    }
    $FiveHour = Get-RemainingPercent $Limits "five_hour"
    if ($null -ne $FiveHour) {
        $Parts.Add(("5h {0:0}%" -f $FiveHour))
    }
    $SevenDay = Get-RemainingPercent $Limits "seven_day"
    if ($null -ne $SevenDay) {
        $Parts.Add(("wk {0:0}%" -f $SevenDay))
    }
    $TotalCost = Get-Field $Cost "total_cost_usd"
    if ($null -ne $TotalCost) {
        $Parts.Add(('$' + ("{0:0.00}" -f [double]$TotalCost)))
    }
    Write-Output ($Parts -join " | ")
}
catch {
    # A status line must never interfere with the active Claude session.
    Write-Output ""
    exit 0
}
