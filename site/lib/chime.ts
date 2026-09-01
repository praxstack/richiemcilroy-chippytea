// The collect chime, synthesized exactly as CoinAudio does it in the app:
// a short five-note arcade arpeggio, no downloaded audio.

let context: AudioContext | null = null;
let buffer: AudioBuffer | null = null;

export function getAudioContext(): AudioContext {
  context ??= new AudioContext();
  if (context.state === "suspended") void context.resume();
  return context;
}

function build(ac: AudioContext): AudioBuffer {
  const sampleRate = 22050;
  const duration = 0.67;
  const count = Math.floor(duration * sampleRate);
  const buf = ac.createBuffer(1, count, sampleRate);
  const data = buf.getChannelData(0);
  const notes = [783.99, 987.77, 1174.66, 1567.98, 1975.53];
  for (let i = 0; i < count; i++) {
    const t = i / sampleRate;
    let value = 0;
    for (let n = 0; n < notes.length; n++) {
      const noteT = t - n * 0.09;
      if (noteT >= 0 && noteT < 0.28) {
        const attack = Math.min(1, noteT / 0.004);
        const envelope = attack * Math.exp(-noteT * 18);
        const hz = notes[n];
        value += (Math.sin(noteT * hz * 2 * Math.PI) + Math.sin(noteT * hz * 4 * Math.PI) * 0.22) * envelope * 0.18;
      }
    }
    data[i] = Math.max(-1, Math.min(1, value));
  }
  return buf;
}

export function playChime() {
  try {
    const context = getAudioContext();
    buffer ??= build(context);
    const source = context.createBufferSource();
    source.buffer = buffer;
    const gain = context.createGain();
    // The app writes ±25000 into Int16 and plays at volume 0.5.
    gain.gain.value = (25000 / 32768) * 0.5;
    source.connect(gain);
    gain.connect(context.destination);
    source.start();
  } catch {
    // No audio is fine; the drawing carries the moment.
  }
}
