# Export the character produced by each physical key (scan code) for every
# keyboard layout loaded in the current Windows session. Read-only: it never
# loads, activates or unloads layouts. Dead keys are marked, not resolved.
# Each layout carries its KLID, which tells layouts of one language apart
# (US 00000409 and US-International 00020409 are both LANGID 0409).
param([Parameter(Mandatory = $true)][string]$Output)
$ErrorActionPreference = 'Stop'
$layoutKeys = 'HKLM:\SYSTEM\CurrentControlSet\Control\Keyboard Layouts'
# The KLID of an HKL: a high word below 0xF000 is the KLID's own number; 0xFnnn names the
# "Layout Id" nnn of a variant, found in the registry.
function Get-Klid([int64]$Hkl) {
    $device = ($Hkl -shr 16) -band 0xffff
    if (($device -band 0xf000) -ne 0xf000) { return ('{0:X8}' -f $device) }
    $layoutId = '{0:x4}' -f ($device -band 0x0fff)
    foreach ($key in Get-ChildItem -LiteralPath $layoutKeys) {
        $id = (Get-ItemProperty -LiteralPath $key.PSPath -Name 'Layout Id' -ErrorAction SilentlyContinue).'Layout Id'
        if ($id -and $id.ToLowerInvariant() -eq $layoutId) { return $key.PSChildName.ToUpperInvariant() }
    }
    throw ('No KLID has Layout Id ' + $layoutId + ' (HKL 0x{0:x})' -f $Hkl)
}
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class LayoutExport {
    [DllImport("user32.dll")] public static extern int GetKeyboardLayoutList(int n, IntPtr[] list);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyEx(uint code, uint type, IntPtr hkl);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int ToUnicodeEx(uint vk, uint sc, byte[] state, StringBuilder buf, int size, uint flags, IntPtr hkl);
    public static IntPtr[] Layouts() {
        int n = GetKeyboardLayoutList(0, null);
        var list = new IntPtr[n];
        GetKeyboardLayoutList(n, list);
        return list;
    }
    // Flag 4: do not change the keyboard state (Windows 10 1607+).
    public static string Char(IntPtr hkl, uint sc, bool shift, out bool dead) {
        dead = false;
        uint vk = MapVirtualKeyEx(sc, 3, hkl);
        if (vk == 0) return null;
        var state = new byte[256];
        if (shift) { state[0x10] = 0x80; state[0xA0] = 0x80; }
        var buf = new StringBuilder(8);
        int n = ToUnicodeEx(vk, sc, state, buf, buf.Capacity, 4, hkl);
        if (n < 0) { dead = true; return null; }
        return n == 1 ? buf.ToString(0, 1) : null;
    }
}
'@
$result = [ordered]@{ format = 1; layouts = @() }
foreach ($hkl in [LayoutExport]::Layouts()) {
    $value = [int64]$hkl
    $keys = [ordered]@{}
    # Main alphanumeric block: scan codes 0x02-0x0D, 0x10-0x1B, 0x1E-0x29, 0x2B-0x35, 0x56.
    $codes = @(0x02..0x0D) + @(0x10..0x1B) + @(0x1E..0x29) + @(0x2B..0x35) + @(0x56)
    foreach ($sc in $codes) {
        $deadNormal = $false; $deadShift = $false
        $normal = [LayoutExport]::Char($hkl, [uint32]$sc, $false, [ref]$deadNormal)
        $shifted = [LayoutExport]::Char($hkl, [uint32]$sc, $true, [ref]$deadShift)
        $keys[('0x{0:x2}' -f $sc)] = [ordered]@{
            normal = $normal; shift = $shifted; dead_normal = $deadNormal; dead_shift = $deadShift
        }
    }
    $result.layouts += [ordered]@{
        hkl = ('0x{0:x}' -f $value)
        klid = (Get-Klid $value)
        language_id = ('{0:x4}' -f ($value -band 0xffff))
        keys = $keys
    }
}
$json = $result | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText($Output, $json, [Text.UTF8Encoding]::new($false))
Write-Output ("EXPORTED " + $result.layouts.Count + " layouts")
