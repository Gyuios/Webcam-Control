using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Text.Json;

namespace ControlWebcamBridge;

internal static class Program
{
    private static readonly JsonSerializerOptions Json = new() { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };

    [STAThread]
    private static int Main(string[] args)
    {
        try
        {
            object response = args.FirstOrDefault()?.ToLowerInvariant() switch
            {
                "list" => ListCameras(),
                "controls" when args.Length >= 2 => GetControls(args[1]),
                "set" when args.Length >= 6 => SetControl(args[1], args[2], int.Parse(args[3]), int.Parse(args[4]), bool.Parse(args[5])),
                _ => new BridgeResult(false, "Comando inválido."),
            };
            Console.WriteLine(JsonSerializer.Serialize(response, Json));
            return response is BridgeResult { Ok: false } ? 1 : 0;
        }
        catch (Exception exception)
        {
            Console.WriteLine(JsonSerializer.Serialize(new BridgeResult(false, exception.Message), Json));
            return 1;
        }
    }

    private static IReadOnlyList<CameraInfo> ListCameras()
    {
        var entries = DirectShow.Enumerate();
        try { return entries.Select(camera => new CameraInfo(camera.Path, camera.Name)).ToArray(); }
        finally { DirectShow.ReleaseAll(entries); }
    }

    private static IReadOnlyList<ControlInfo> GetControls(string path)
    {
        CameraEntry? camera = DirectShow.Find(path);
        if (camera is null) throw new InvalidOperationException("La cámara seleccionada ya no está disponible.");
        try
        {
            object filter = camera.Bind();
            try
            {
                var controls = new List<ControlInfo>();
                IAMCameraControl? cameraControl = DirectShow.GetInterface<IAMCameraControl>(filter);
                IAMVideoProcAmp? videoControl = DirectShow.GetInterface<IAMVideoProcAmp>(filter);
                try
                {
                    if (cameraControl is not null)
                        AddCameraControls(controls, cameraControl);
                    if (videoControl is not null)
                        AddVideoControls(controls, videoControl);
                }
                finally
                {
                    DirectShow.Release(videoControl);
                    DirectShow.Release(cameraControl);
                }
                return controls;
            }
            finally { DirectShow.Release(filter); }
        }
        finally { DirectShow.Release(camera.Moniker); }
    }

    private static BridgeResult SetControl(string path, string kind, int property, int value, bool automatic)
    {
        CameraEntry? camera = DirectShow.Find(path);
        if (camera is null) return new BridgeResult(false, "La cámara seleccionada ya no está disponible.");
        try
        {
            object filter = camera.Bind();
            try
            {
                ControlFlags flags = automatic ? ControlFlags.Auto : ControlFlags.Manual;
                int result = kind.Equals("camera", StringComparison.OrdinalIgnoreCase)
                    ? DirectShow.GetInterface<IAMCameraControl>(filter)?.Set((CameraControlProperty)property, value, flags) ?? unchecked((int)0x80004005)
                    : DirectShow.GetInterface<IAMVideoProcAmp>(filter)?.Set((VideoProcAmpProperty)property, value, flags) ?? unchecked((int)0x80004005);
                return result >= 0 ? new BridgeResult(true, null) : new BridgeResult(false, $"El controlador rechazó el cambio (0x{result:X8}).");
            }
            finally { DirectShow.Release(filter); }
        }
        finally { DirectShow.Release(camera.Moniker); }
    }

    private static void AddCameraControls(List<ControlInfo> controls, IAMCameraControl control)
    {
        foreach (var item in new[]
        {
            (CameraControlProperty.Exposure, "exposure", "Exposición"),
            (CameraControlProperty.Zoom, "zoom", "Zoom"),
            (CameraControlProperty.Iris, "iris", "Iris"),
            (CameraControlProperty.Focus, "focus", "Enfoque"),
        })
        {
            if (control.GetRange(item.Item1, out int min, out int max, out int step, out int defaultValue, out ControlFlags caps) < 0) continue;
            control.Get(item.Item1, out int value, out ControlFlags flags);
            controls.Add(new ControlInfo(item.Item2, item.Item3, "camera", (int)item.Item1, min, max, step, defaultValue, value, (flags & ControlFlags.Auto) != 0, (caps & ControlFlags.Auto) != 0));
        }
    }

    private static void AddVideoControls(List<ControlInfo> controls, IAMVideoProcAmp control)
    {
        foreach (var item in new[]
        {
            (VideoProcAmpProperty.Brightness, "brightness", "Brillo"),
            (VideoProcAmpProperty.Contrast, "contrast", "Contraste"),
            (VideoProcAmpProperty.Hue, "hue", "Tono"),
            (VideoProcAmpProperty.Saturation, "saturation", "Saturación"),
            (VideoProcAmpProperty.Sharpness, "sharpness", "Nitidez"),
            (VideoProcAmpProperty.Gamma, "gamma", "Gamma"),
            (VideoProcAmpProperty.WhiteBalance, "whiteBalance", "Balance de blancos"),
            (VideoProcAmpProperty.BacklightCompensation, "backlight", "Luz de fondo"),
            (VideoProcAmpProperty.Gain, "gain", "Ganancia"),
        })
        {
            if (control.GetRange(item.Item1, out int min, out int max, out int step, out int defaultValue, out ControlFlags caps) < 0) continue;
            control.Get(item.Item1, out int value, out ControlFlags flags);
            controls.Add(new ControlInfo(item.Item2, item.Item3, "video", (int)item.Item1, min, max, step, defaultValue, value, (flags & ControlFlags.Auto) != 0, (caps & ControlFlags.Auto) != 0));
        }
    }
}

internal sealed record CameraInfo(string Id, string Name);
internal sealed record ControlInfo(string Id, string Name, string Kind, int Property, int Minimum, int Maximum, int Step, int DefaultValue, int Value, bool Automatic, bool SupportsAuto);
internal sealed record BridgeResult(bool Ok, string? Error);
internal sealed record CameraEntry(string Name, string Path, IMoniker Moniker)
{
    internal object Bind()
    {
        Guid iid = typeof(IBaseFilter).GUID;
        Moniker.BindToObject(null!, null!, ref iid, out object filter);
        return filter;
    }
}

internal static class DirectShow
{
    private static readonly Guid VideoInputCategory = new("860BB310-5D01-11D0-BD3B-00A0C911CE86");

    internal static List<CameraEntry> Enumerate()
    {
        var cameras = new List<CameraEntry>();
        var enumerator = (ICreateDevEnum)new CreateDevEnum();
        try
        {
            Guid category = VideoInputCategory;
            if (enumerator.CreateClassEnumerator(ref category, out IEnumMoniker? monikers, 0) != 0 || monikers is null) return cameras;
            try
            {
                var current = new IMoniker[1];
                while (monikers.Next(1, current, IntPtr.Zero) == 0)
                {
                    cameras.Add(new CameraEntry(FriendlyName(current[0]), DevicePath(current[0]), current[0]));
                    current = new IMoniker[1];
                }
            }
            finally { Release(monikers); }
        }
        finally { Release(enumerator); }
        return cameras;
    }

    internal static CameraEntry? Find(string path)
    {
        List<CameraEntry> cameras = Enumerate();
        CameraEntry? selected = cameras.FirstOrDefault(camera => string.Equals(camera.Path, path, StringComparison.OrdinalIgnoreCase));
        foreach (CameraEntry camera in cameras)
            if (!ReferenceEquals(camera, selected)) Release(camera.Moniker);
        return selected;
    }

    internal static void ReleaseAll(IEnumerable<CameraEntry> cameras)
    {
        foreach (CameraEntry camera in cameras) Release(camera.Moniker);
    }

    internal static T? GetInterface<T>(object source) where T : class
    {
        IntPtr unknown = IntPtr.Zero;
        IntPtr requested = IntPtr.Zero;
        try
        {
            unknown = Marshal.GetIUnknownForObject(source);
            Guid iid = typeof(T).GUID;
            int hr = Marshal.QueryInterface(unknown, ref iid, out requested);
            return hr < 0 ? null : (T)Marshal.GetObjectForIUnknown(requested);
        }
        finally
        {
            if (requested != IntPtr.Zero) Marshal.Release(requested);
            if (unknown != IntPtr.Zero) Marshal.Release(unknown);
        }
    }

    internal static void Release(object? item)
    {
        if (item is not null && Marshal.IsComObject(item))
            try { Marshal.ReleaseComObject(item); } catch { }
    }

    private static string FriendlyName(IMoniker moniker) => ReadProperty(moniker, "FriendlyName", "Webcam sin nombre");
    private static string DevicePath(IMoniker moniker) => ReadProperty(moniker, "DevicePath", string.Empty);
    private static string ReadProperty(IMoniker moniker, string name, string fallback)
    {
        object? bagObject = null;
        try
        {
            Guid iid = typeof(IPropertyBag).GUID;
            moniker.BindToStorage(null!, null!, ref iid, out bagObject);
            object value = fallback;
            return ((IPropertyBag)bagObject).Read(name, ref value, IntPtr.Zero) >= 0 ? Convert.ToString(value) ?? fallback : fallback;
        }
        catch { return fallback; }
        finally { Release(bagObject); }
    }
}

[ComImport, Guid("62BE5D10-60EB-11D0-BD3B-00A0C911CE86")]
internal class CreateDevEnum { }
[ComImport, Guid("29840822-5B84-11D0-BD3B-00A0C911CE86"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface ICreateDevEnum { [PreserveSig] int CreateClassEnumerator(ref Guid category, [MarshalAs(UnmanagedType.Interface)] out IEnumMoniker? enumerator, int flags); }
[ComImport, Guid("56A86895-0AD4-11CE-B03A-0020AF0BA770"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IBaseFilter { }
[ComImport, Guid("55272A00-42CB-11CE-8135-00AA004BB851"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IPropertyBag { [PreserveSig] int Read([MarshalAs(UnmanagedType.LPWStr)] string propertyName, [MarshalAs(UnmanagedType.Struct)] ref object value, IntPtr errorLog); [PreserveSig] int Write([MarshalAs(UnmanagedType.LPWStr)] string propertyName, [MarshalAs(UnmanagedType.Struct)] ref object value); }
[ComImport, Guid("C6E13360-30AC-11D0-A18C-00A0C9118956"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IAMVideoProcAmp { [PreserveSig] int GetRange(VideoProcAmpProperty property, out int min, out int max, out int steppingDelta, out int defaultValue, out ControlFlags capabilities); [PreserveSig] int Set(VideoProcAmpProperty property, int value, ControlFlags flags); [PreserveSig] int Get(VideoProcAmpProperty property, out int value, out ControlFlags flags); }
[ComImport, Guid("C6E13370-30AC-11D0-A18C-00A0C9118956"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
internal interface IAMCameraControl { [PreserveSig] int GetRange(CameraControlProperty property, out int min, out int max, out int steppingDelta, out int defaultValue, out ControlFlags capabilities); [PreserveSig] int Set(CameraControlProperty property, int value, ControlFlags flags); [PreserveSig] int Get(CameraControlProperty property, out int value, out ControlFlags flags); }
[Flags] internal enum ControlFlags { None = 0, Auto = 1, Manual = 2 }
internal enum VideoProcAmpProperty { Brightness, Contrast, Hue, Saturation, Sharpness, Gamma, ColorEnable, WhiteBalance, BacklightCompensation, Gain }
internal enum CameraControlProperty { Pan, Tilt, Roll, Zoom, Exposure, Iris, Focus, ScanMode, Privacy }
