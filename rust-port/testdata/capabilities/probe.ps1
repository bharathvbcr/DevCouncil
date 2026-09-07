function Get-Widget {
    param([string]$Name)
    return $Name
}

function Invoke-Probe {
    Get-Widget -Name "x"
}
