# Has Excel write a copy of a workbook that xldif can read. Run by
# xldif-cli (src/excel.rs), which passes everything in the environment:
#
# XLDIF_EXCEL_MODE  "convert": open XLDIF_EXCEL_IN in a new, hidden Excel
#                   and save it as .xlsx to XLDIF_EXCEL_OUT.
#                   "snapshot": find XLDIF_EXCEL_IN open in a running Excel
#                   and save a copy of what it holds now, unsaved edits
#                   included, to XLDIF_EXCEL_OUT. The open workbook is not
#                   saved, closed, or changed.
# XLDIF_PASSWORD    optional, to open a password-protected workbook
#                   (convert only).
#
# Prints one line: "ok", or "<reason><tab><message>" with reason one of
# not_open, busy, failed. ASCII only: Windows PowerShell reads a script
# without a byte-order mark in the system code page.
[Console]::OutputEncoding = New-Object Text.UTF8Encoding $false
$ErrorActionPreference = 'Stop'
$in = $env:XLDIF_EXCEL_IN
$out = $env:XLDIF_EXCEL_OUT

function Release($o) {
    if ($null -ne $o) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($o) }
}

function Fail($reason, $message) {
    [Console]::Out.WriteLine("$reason`t" + ($message -replace '\s+', ' '))
    exit 0
}

if ($env:XLDIF_EXCEL_MODE -eq 'snapshot') {
    # Looks the file up among the objects running programs have registered.
    # Asking for it by name instead would make Excel load a file that is not
    # open, inside whichever Excel is running.
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
public static class XldifRunning {
    [DllImport("ole32.dll")] static extern int GetRunningObjectTable(int reserved, out IRunningObjectTable table);
    [DllImport("ole32.dll")] static extern int CreateBindCtx(int reserved, out IBindCtx context);
    public static object Find(string path) {
        IRunningObjectTable table; IBindCtx context; IEnumMoniker all;
        if (GetRunningObjectTable(0, out table) != 0 || CreateBindCtx(0, out context) != 0) return null;
        table.EnumRunning(out all);
        IMoniker[] one = new IMoniker[1];
        while (all.Next(1, one, IntPtr.Zero) == 0) {
            string name;
            try { one[0].GetDisplayName(context, null, out name); } catch (Exception) { continue; }
            if (string.Equals(name, path, StringComparison.OrdinalIgnoreCase)) {
                object found;
                return table.GetObject(one[0], out found) == 0 ? found : null;
            }
        }
        return null;
    }
}
"@
    $wb = $null
    try {
        $wb = [XldifRunning]::Find($in)
        if ($null -eq $wb) { Fail 'not_open' 'no running Excel has the workbook open under this path' }
        if ($wb.FullName -ne $in) { Fail 'not_open' 'no running Excel has the workbook open under this path' }
        $wb.SaveCopyAs($out)
    } catch [Runtime.InteropServices.COMException] {
        # Excel will not take calls while a cell is being edited or a dialog
        # is open.
        $code = '{0:X8}' -f $_.Exception.ErrorCode
        if ($code -eq '800AC472' -or $code -eq '80010001' -or $code -eq '8001010A') {
            Fail 'busy' 'Excel is busy: finish editing the cell or close its dialog, then try again'
        }
        Fail 'failed' $_.Exception.Message
    } catch {
        Fail 'failed' $_.Exception.Message
    } finally {
        Release $wb
    }
    [Console]::Out.WriteLine('ok')
    exit 0
}

$xl = $null
$wb = $null
try {
    $xl = New-Object -ComObject Excel.Application
    $xl.Visible = $false
    $xl.DisplayAlerts = $false
    $xl.AskToUpdateLinks = $false
    $xl.EnableEvents = $false
    $xl.AutomationSecurity = 3   # macros stay disabled
    $missing = [Type]::Missing
    $password = if ($env:XLDIF_PASSWORD) { $env:XLDIF_PASSWORD } else { '' }
    # UpdateLinks 0, ReadOnly; a wrong or missing password fails instead of asking.
    $wb = $xl.Workbooks.Open($in, 0, $true, $missing, $password)
    # Excel opens nearly any file, as text if nothing else. Only its own
    # workbook formats count: .xlsx 51, .xlsm 52, .xlsb 50, .xls 56 and
    # -4143, and the older .xls of Excel 5 and 95 (39, 43).
    if (@(51, 52, 50, 56, -4143, 39, 43) -notcontains [int]$wb.FileFormat) {
        Fail 'failed' 'Excel does not read it as a workbook'
    }
    # 51 is .xlsx; the copy carries no password.
    $wb.SaveAs($out, 51, '')
} catch {
    Fail 'failed' $_.Exception.Message
} finally {
    if ($null -ne $wb) { try { $wb.Close($false) } catch {} }
    if ($null -ne $xl) { try { $xl.Quit() } catch {} }
    Release $wb
    Release $xl
    [GC]::Collect()
    [GC]::WaitForPendingFinalizers()
}
[Console]::Out.WriteLine('ok')
