// Decodes an uploaded file via the Web Audio API and downmixes to mono
// f32 PCM, mirroring tempo-cli's decode.rs (channel-averaged mono). Browser
// decoding does not do the CLI's gapless MP3 encoder-delay trim, so offsets
// on some MP3s may be shifted a few tens of ms later than the CLI's.

export interface DecodedAudio {
  samples: Float32Array;
  sampleRate: number;
  durationSeconds: number;
}

export async function decodeToMono(
  file: File,
  startSeconds = 0,
  maxDurationSeconds?: number,
): Promise<DecodedAudio> {
  const arrayBuffer = await file.arrayBuffer();
  const AudioContextCtor =
    window.AudioContext ||
    (window as unknown as { webkitAudioContext: typeof AudioContext })
      .webkitAudioContext;
  const audioCtx = new AudioContextCtor();
  let audioBuffer: AudioBuffer;
  try {
    audioBuffer = await audioCtx.decodeAudioData(arrayBuffer);
  } finally {
    await audioCtx.close();
  }

  const { numberOfChannels, sampleRate, length } = audioBuffer;
  const channels: Float32Array[] = [];
  for (let ch = 0; ch < numberOfChannels; ch++) {
    channels.push(audioBuffer.getChannelData(ch));
  }

  const startSample = Math.max(0, Math.floor(startSeconds * sampleRate));
  const endSample =
    maxDurationSeconds && maxDurationSeconds > 0
      ? Math.min(length, startSample + Math.floor(maxDurationSeconds * sampleRate))
      : length;

  const frameCount = Math.max(0, endSample - startSample);
  const mono = new Float32Array(frameCount);
  for (let i = 0; i < frameCount; i++) {
    let sum = 0;
    const srcIndex = startSample + i;
    for (let ch = 0; ch < numberOfChannels; ch++) sum += channels[ch][srcIndex];
    mono[i] = sum / numberOfChannels;
  }

  return { samples: mono, sampleRate, durationSeconds: frameCount / sampleRate };
}
