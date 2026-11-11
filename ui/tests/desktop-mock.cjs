// Test-only native bridge. Never imported by the production UI.
function installDesktopMock() {
  localStorage.setItem('airsend.lang', 'en');
  const callbacks = new Map(), listeners = new Map();
  let next = 0, generation = 0, streaming = false;
  let latency = 500;
  const devices = ['Office', 'Living Room', 'Kitchen'].map((name, i) => ({
    id: `${name}._airplay._tcp.local.`, hardware_id: `aabbccddee0${i}`,
    name, host: `${name.toLowerCase().replaceAll(' ', '-')}.local.`,
    addresses: [`192.0.2.${10+i}`], port: 7000, kind: 'homepod',
    model: 'AudioAccessory11,1', features: null, supports_airplay2: true,
    available: true, manual: false,
  }));
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {unregisterListener() {}};
  window.__mockCalls = [];
  window.__mockFailConnection = false;
  window.__mockEmit = (event, payload) => {
    for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({event, id, payload});
  };
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) { const id = ++next; callbacks.set(id, callback); return id; },
    unregisterCallback(id) { callbacks.delete(id); },
    async invoke(command, args = {}) {
      window.__mockCalls.push({command, args});
      if (command === 'plugin:event|listen') {
        const entries = listeners.get(args.event) ?? [];
        listeners.set(args.event, [...entries, args.handler]); return args.handler;
      }
      if (command === 'plugin:event|unlisten') return null;
      if (command === 'start_discovery_stream') {
        setTimeout(() => devices.forEach(d => window.__mockEmit('airplay://device',d)), 20);
        return null;
      }
      if (command === 'get_last_device') return null;
      if (command === 'get_volume') return .26;
      if (command === 'get_latency') return latency;
      if (command === 'get_latency_cooldown_ms') return 0;
      if (command === 'get_experimental_local_buffer') return true;
      if (command === 'get_multi_device') return false;
      if (command === 'get_audio_options') return {device_id:'virtual-output',mute_local:false,legacy_timing:false};
      if (command === 'list_audio_outputs') return [{id:'virtual-output',name:'CABLE Input (VB-Audio Virtual Cable)',is_default:true}];
      if (command === 'get_receiver_preferences') return {volume:.26,latency_ms:latency,reconnect:true};
      if (command === 'get_session_status') return {generation,status:streaming?'streaming':'disconnected',mute_status:'off',group_mode:false};
      if (command === 'get_receiver_volumes') return [];
      if (command === 'connect_device') {
        if(window.__mockFailConnection) throw Error('Receiver did not respond');
        return {ip:devices[0].addresses[0],port:7000,name:devices[0].name};
      }
      if (command === 'start_streaming') {
        streaming=true;generation++;
        window.__mockEmit('airplay://session',{generation,code:'streaming'});
        return {ip:devices[0].addresses[0],port:7000,name:devices[0].name,sample_rate:44100,channels:2};
      }
      if (command === 'confirm_latency') {latency=args.latencyMs;return streaming;}
      if (command === 'stop_streaming') {streaming=false;generation++;return null;}
      if (command === 'is_streaming') return streaming ? {} : null;
      if (command === 'export_audio_diagnostics') return JSON.stringify({schema_version:1,sessions:[]});
      if (command.startsWith('save_') || command.startsWith('set_') || command==='disconnect_device' || command==='patch_receiver_preferences' || command==='open_latest_release') return null;
      throw Error(`Unmocked native command: ${command}`);
    },
  };
}
module.exports = {installDesktopMock};
