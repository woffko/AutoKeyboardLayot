param(
    [Parameter(Mandatory=$true)][ValidateSet('probe','create','verify')][string]$Mode,
    [Parameter(Mandatory=$true)][string]$Operation,
    [string]$PublicKey = '',
    [string]$Fingerprint = ''
)
$ErrorActionPreference = 'Stop'
# Secret input is read inside C#, not through PowerShell cmdlet parameters or strings.
Add-Type -ReferencedAssemblies System.Security -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Text.RegularExpressions;

public static class PackageKeyVault {
    const string Domain = "AutoKeyboardLayot.package-signing.dpapi.v1";
    [StructLayout(LayoutKind.Sequential)]
    struct SecurityAttributes { public int Length; public IntPtr Descriptor; public int Inherit; }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool CreateDirectoryW(string path, ref SecurityAttributes attributes);

    static void NoReparse(string path) {
        for (string p = Path.GetFullPath(path); !String.IsNullOrEmpty(p); p = Path.GetDirectoryName(p)) {
            if (Directory.Exists(p) || File.Exists(p)) {
                if ((File.GetAttributes(p) & FileAttributes.ReparsePoint) != 0) throw new Exception();
            }
        }
    }
    static void CheckAcl(string path, SecurityIdentifier sid) {
        NoReparse(path);
        DirectorySecurity acl = Directory.GetAccessControl(path);
        if (!acl.AreAccessRulesProtected || !acl.GetOwner(typeof(SecurityIdentifier)).Equals(sid)) throw new Exception();
        foreach (FileSystemAccessRule rule in acl.GetAccessRules(true, true, typeof(SecurityIdentifier))) {
            string who = rule.IdentityReference.Value;
            if (rule.AccessControlType == AccessControlType.Allow && who != sid.Value && who != "S-1-5-18") throw new Exception();
        }
    }
    static void NewDirectory(string path, SecurityIdentifier sid) {
        NoReparse(path);
        RawSecurityDescriptor descriptor = new RawSecurityDescriptor("O:" + sid.Value + "D:P(A;OICI;FA;;;" + sid.Value + ")(A;OICI;FA;;;SY)");
        byte[] raw = new byte[descriptor.BinaryLength];
        descriptor.GetBinaryForm(raw, 0);
        GCHandle pinned = GCHandle.Alloc(raw, GCHandleType.Pinned);
        try {
            SecurityAttributes attributes = new SecurityAttributes();
            attributes.Length = Marshal.SizeOf(typeof(SecurityAttributes));
            attributes.Descriptor = pinned.AddrOfPinnedObject();
            // Atomic exclusive directory creation with the restrictive ACL already set.
            if (!CreateDirectoryW(path, ref attributes)) throw new Exception();
        } finally { pinned.Free(); }
        CheckAcl(path, sid);
    }
    static byte[] Input(int size) {
        byte[] bytes = new byte[size];
        try {
            Stream stream = Console.OpenStandardInput();
            int position = 0;
            while (position < size) {
                int count = stream.Read(bytes, position, size-position);
                if (count == 0) throw new Exception();
                position += count;
            }
            if (stream.ReadByte() != -1) throw new Exception();
            return bytes;
        } catch { Array.Clear(bytes, 0, bytes.Length); throw; }
    }
    static bool Equal(byte[] a, byte[] b) {
        if (a.Length != b.Length) return false;
        int different = 0;
        for (int i=0; i<a.Length; i++) different |= a[i] ^ b[i];
        return different == 0;
    }
    static void WriteNew(string path, byte[] bytes) {
        using (FileStream stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None)) {
            stream.Write(bytes, 0, bytes.Length);
            stream.Flush(true);
        }
    }
    static byte[] Unprotect(string path, byte[] entropy) {
        NoReparse(path);
        if (new FileInfo(path).Length > 65536) throw new Exception();
        return ProtectedData.Unprotect(File.ReadAllBytes(path), entropy, DataProtectionScope.CurrentUser);
    }
    public static int Run(string mode, string operation, string publicKey, string fingerprint) {
        byte[] input = null, plain = null;
        string stage = "preflight";
        try {
            Console.OutputEncoding = new UTF8Encoding(false);
            if (!Regex.IsMatch(operation, "^[a-z0-9-]{1,32}$")) throw new Exception();
            WindowsIdentity identity = WindowsIdentity.GetCurrent();
            if (identity.IsSystem || identity.IsAnonymous || identity.User == null ||
                identity.User.IsWellKnown(WellKnownSidType.LocalServiceSid) ||
                identity.User.IsWellKnown(WellKnownSidType.NetworkServiceSid)) throw new Exception();
            string local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            if (String.IsNullOrEmpty(local) || !Path.IsPathRooted(local)) throw new Exception();
            string root = Path.Combine(local, "AutoKeyboardLayot-Signing");
            string directory = Path.Combine(root, operation);
            NoReparse(directory);
            if (Directory.Exists(root)) CheckAcl(root, identity.User);
            if (mode == "probe") {
                byte[] sample = Encoding.ASCII.GetBytes("public DPAPI preflight fixture");
                byte[] encoded = ProtectedData.Protect(sample, null, DataProtectionScope.CurrentUser);
                plain = ProtectedData.Unprotect(encoded, null, DataProtectionScope.CurrentUser);
                if (!Equal(sample, plain) || Directory.Exists(directory) || File.Exists(directory)) throw new Exception();
                string scratch = Path.Combine(Path.GetTempPath(), "AutoKeyboardLayot-KeyAclProbe-" + Guid.NewGuid().ToString("N"));
                NewDirectory(scratch, identity.User);
                // Only our successfully created, empty ACL-test directory.
                Directory.Delete(scratch);
                Console.WriteLine("READY"); Console.WriteLine(directory); return 0;
            }
            if (!Regex.IsMatch(publicKey, "^[a-f0-9]{64}$") || !Regex.IsMatch(fingerprint, "^[a-f0-9]{64}$")) throw new Exception();
            byte[] pub = new byte[32];
            for (int i=0; i<32; i++) pub[i] = Convert.ToByte(publicKey.Substring(i*2,2),16);
            using (SHA256 hash = SHA256.Create()) {
                if (BitConverter.ToString(hash.ComputeHash(pub)).Replace("-", "").ToLowerInvariant() != fingerprint) throw new Exception();
            }
            byte[] entropy = Encoding.UTF8.GetBytes(Domain + "\0" + operation + "\0" + publicKey);
            string keyPath = Path.Combine(directory, "private.pkcs8.dpapi");
            if (mode == "create") {
                stage = "create-directory";
                if (!Directory.Exists(root)) NewDirectory(root, identity.User);
                NewDirectory(directory, identity.User);
                stage = "read-key";
                input = Input(48);
                byte[] prefix = new byte[] {0x30,0x2e,0x02,0x01,0x00,0x30,0x05,0x06,0x03,0x2b,0x65,0x70,0x04,0x22,0x04,0x20};
                for (int i=0;i<prefix.Length;i++) if(input[i]!=prefix[i]) throw new Exception();
                stage = "protect-write";
                byte[] ciphertext = ProtectedData.Protect(input, entropy, DataProtectionScope.CurrentUser);
                WriteNew(keyPath, ciphertext);
                stage = "read-back";
                plain = Unprotect(keyPath, entropy);
                if (!Equal(input, plain)) throw new Exception();
                string metadata = "{\"format\":1,\"algorithm\":\"Ed25519\",\"signer\":\"" + operation + "\",\"public_key_hex\":\"" + publicKey + "\",\"fingerprint_sha256\":\"" + fingerprint + "\",\"protection\":\"DPAPI-CurrentUser-PKCS8\",\"entropy_domain\":\"" + Domain + "\"}";
                WriteNew(Path.Combine(directory, "public.json"), Encoding.UTF8.GetBytes(metadata));
            } else if (mode == "verify") {
                stage = "verify-stored-key";
                CheckAcl(directory, identity.User);
                input = Input(32);
                plain = Unprotect(keyPath, entropy);
                using (SHA256 hash = SHA256.Create()) { if (!Equal(input, hash.ComputeHash(plain))) throw new Exception(); }
            } else throw new Exception();
            Console.WriteLine(mode == "create" ? "CREATED" : "VERIFIED");
            Console.WriteLine(directory);
            return 0;
        } catch {
            // Never print exception objects, secret input, or local variables.
            Console.Error.WriteLine("KEY_OPERATION_FAILED stage=" + stage);
            return 1;
        } finally {
            if (input != null) Array.Clear(input,0,input.Length);
            if (plain != null) Array.Clear(plain,0,plain.Length);
        }
    }
}
'@
exit ([PackageKeyVault]::Run($Mode, $Operation, $PublicKey, $Fingerprint))
