using System;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Threading.Tasks;

namespace AirSend;

internal static class NativeEngine
{
    [DllImport("airsend_engine", EntryPoint = "airsend_request", CallingConvention = CallingConvention.Cdecl)]
    private static extern IntPtr Request([MarshalAs(UnmanagedType.LPUTF8Str)] string request);

    [DllImport("airsend_engine", EntryPoint = "airsend_free", CallingConvention = CallingConvention.Cdecl)]
    private static extern void Free(IntPtr response);

    internal static Task<JsonElement> Invoke(object command) => Task.Run(() =>
    {
        var response = Request(JsonSerializer.Serialize(command));
        try
        {
            using var document = JsonDocument.Parse(Marshal.PtrToStringUTF8(response)
                ?? throw new InvalidOperationException("Missing engine response"));
            if (document.RootElement.TryGetProperty("error", out var error))
                throw new InvalidOperationException(error.GetString());
            return document.RootElement.GetProperty("data").Clone();
        }
        finally
        {
            Free(response);
        }
    });
}
