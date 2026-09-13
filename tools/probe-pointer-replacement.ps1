param([Parameter(Mandatory=$true)][string]$Receipt)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $Receipt) { throw 'Use a new receipt.' }
Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class PointerReplacementProbe {
  [StructLayout(LayoutKind.Sequential)] struct RenameInfo { public uint Flags; public IntPtr Root; public uint Length; public ushort First; }
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool MoveFileEx(string from,string to,uint flags);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool ReplaceFile(string to,string from,string backup,uint flags,IntPtr a,IntPtr b);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern SafeFileHandle CreateFile(string path,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
  [DllImport("kernel32.dll", SetLastError=true)] static extern bool SetFileInformationByHandle(SafeFileHandle file,int info,IntPtr buffer,uint size);
  public sealed class Result { public string api; public bool success; public int win32_error; public string old_handle; public string new_open; public bool source_exists; }
  public static Result Run(string parent,string api) {
    string directory=Path.Combine(parent,api); Directory.CreateDirectory(directory);
    string target=Path.Combine(directory,"CURRENT"), source=Path.Combine(directory,"new-pointer");
    File.WriteAllText(target,"old-v1",new UTF8Encoding(false));
    using(var stream=new FileStream(source,FileMode.CreateNew,FileAccess.Write,FileShare.None)) {
      byte[] bytes=Encoding.UTF8.GetBytes("new-v2"); stream.Write(bytes,0,bytes.Length); stream.Flush(true);
    }
    var result=new Result {api=api};
    using(var old=new FileStream(target,FileMode.Open,FileAccess.Read,FileShare.Read|FileShare.Delete)) {
      if(api=="MoveFileEx") result.success=MoveFileEx(source,target,1|8);
      else if(api=="ReplaceFile") result.success=ReplaceFile(target,source,null,0,IntPtr.Zero,IntPtr.Zero);
      else {
        using(var handle=CreateFile(source,0x10000|0x80,1|4,IntPtr.Zero,3,0x200000,IntPtr.Zero)) {
          if(handle.IsInvalid) throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error());
          byte[] name=Encoding.Unicode.GetBytes(target);
          int offset=Marshal.OffsetOf(typeof(RenameInfo),"First").ToInt32();
          int size=checked(Marshal.SizeOf(typeof(RenameInfo))+name.Length+2);
          IntPtr memory=Marshal.AllocHGlobal(size);
          try {
            Marshal.Copy(new byte[size],0,memory,size);
            Marshal.StructureToPtr(new RenameInfo {Flags=3,Root=IntPtr.Zero,Length=(uint)name.Length},memory,false);
            Marshal.Copy(name,0,IntPtr.Add(memory,offset),name.Length);
            result.success=SetFileInformationByHandle(handle,22,memory,(uint)size);
            result.win32_error=result.success?0:Marshal.GetLastWin32Error();
          } finally { Marshal.FreeHGlobal(memory); }
        }
      }
      if(api!="FileRenameInfoEx") result.win32_error=result.success?0:Marshal.GetLastWin32Error();
      using(var reader=new StreamReader(old,Encoding.UTF8)) result.old_handle=reader.ReadToEnd();
      result.new_open=File.ReadAllText(target,Encoding.UTF8);
      result.source_exists=File.Exists(source);
    }
    return result;
  }
}
'@
$root = Join-Path ([IO.Path]::GetTempPath()) ('AutoKeyboardLayot-pointer-probe-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$results = @('MoveFileEx','ReplaceFile','FileRenameInfoEx') | ForEach-Object { [PointerReplacementProbe]::Run($root, $_) }
$result = @{fixture_root=$root; results=$results; live_store_modified=$false; power_loss_test=$false}
$json = $result | ConvertTo-Json -Depth 5
$stream = [IO.File]::Open($Receipt, [IO.FileMode]::CreateNew)
try { $bytes=[Text.Encoding]::UTF8.GetBytes($json); $stream.Write($bytes,0,$bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
Write-Output $json
