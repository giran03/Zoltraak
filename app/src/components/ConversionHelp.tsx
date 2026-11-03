import { useEffect, useId, useRef, useState } from "react";
import { Button } from "./Controls";
import { Close, Info } from "./Icons";
import { TooltipGroup } from "./Tooltip";

const HELP = {
  dithering: {
    title: "GIF dithering",
    summary: "GIF uses a limited palette of colors. Dithering mixes tiny dots of those colors to make shading and gradients look smoother.",
    items: [
      ["Smooth · Sierra", "Spreads color differences into nearby pixels. A good starting point for video, photos, and soft gradients; it can add a grainy texture."],
      ["Ordered · Bayer", "Uses a repeating dot pattern. Choose it for a consistent, patterned texture; the pattern may be visible in smooth areas."],
      ["None", "Adds no dithering dots. Useful for flat-color artwork and logos; gradients may show visible steps or bands."],
    ],
    note: "There is no single best choice for every clip. Try a short range to compare appearance and file size. Dithering does not change the frame rate or loop count.",
  },
  processing: {
    title: "Copying and encoding",
    summary: "A container is the outer file format, such as MP4 or MKV. A codec is how the picture or audio inside it is compressed.",
    items: [
      ["Automatic", "Copies compatible streams when no processing is needed. Otherwise, it encodes the streams that need changes."],
      ["Encode", "Rebuilds picture or audio using your settings. Choose this to apply quality or bitrate changes, including conversion to the same format."],
      ["Stream copy only", "Preserves compressed streams without re-encoding. Fast and avoids another compression step, but cannot resize, apply fades, or make other changes that need encoding."],
    ],
    note: "The review list describes the actual operation for each file before you start.",
  },
  format: {
    title: "Formats and codecs",
    summary: "The output format determines what the resulting file can contain and which settings are available.",
    items: [
      ["Video", "MP4, MKV, MOV, and WebM hold picture and audio streams. Encoded MP4/MKV/MOV use H.264 picture and AAC audio here; encoded WebM uses VP9 and Opus."],
      ["Images and animations", "JPEG, PNG, BMP, and TIFF export one image. GIF and WebP can export animation. Still-image clips need a duration."],
      ["Audio", "MP3, AAC, Opus, and Vorbis compress audio with some detail loss. FLAC, ALAC, WAV, and AIFF offer lossless output, but cannot recover detail already lost in the source."],
    ],
    note: "Only outputs supported by your installed media tools are offered. M4A (AAC) and M4A (Apple Lossless) share an extension but use different codecs.",
  },
  quality: {
    title: "Quality and compression",
    summary: "Quality controls how much detail an encoder keeps. Higher quality usually needs more space, but the scale depends on the output format.",
    items: [
      ["Video, JPEG, and lossy WebP", "Higher values favor detail over file size. Choose Encode if Automatic would otherwise copy an existing stream."],
      ["PNG compression effort", "Changes how hard the encoder works to pack the same pixels. It affects speed and size, not visual quality."],
      ["Lossless WebP", "Preserves the decoded pixels instead of using lossy compression. This cannot undo earlier losses in the source."],
    ],
    note: "The same quality number is not equivalent across different codecs. A higher number does not increase the source's resolution or restore missing detail.",
  },
  dimensions: {
    title: "Dimensions and aspect ratio",
    summary: "Width and height are measured in pixels. These fields set a maximum size while keeping the picture's original proportions.",
    items: [
      ["One dimension", "Set only width or height and the other dimension is calculated automatically."],
      ["Both dimensions", "The picture fits inside that box without stretching. Its actual dimensions may be smaller."],
      ["Keep the source", "Leave both empty. Smaller inputs are not enlarged; some video codecs require padding to even dimensions."],
    ],
    note: "Resizing requires encoding. It changes picture size, not the length of the clip.",
  },
  fps: {
    title: "Frame rate (fps)",
    summary: "Frames per second is how many pictures are displayed during each second of an animation or video.",
    items: [
      ["Lower frame rate", "Usually makes animations smaller, but motion can look less smooth."],
      ["Higher frame rate", "Keeps more frames when available. Raising it above the source does not create new motion detail."],
      ["Keep source timing", "Leave the field empty. GIF timing has limited precision, so its effective frame rate may be approximate."],
    ],
    note: "Changing frame rate here does not change playback speed or clip duration.",
  },
  bitrate: {
    title: "Audio bitrate (kbps)",
    summary: "Kilobits per second measures how much data is allocated to each second of compressed audio.",
    items: [
      ["Higher bitrate", "Usually keeps more detail and produces a larger file. It cannot restore detail missing from the source."],
      ["Lower bitrate", "Saves space but can make music sound less clear or introduce audible artifacts."],
      ["When it applies", "Used when encoding lossy audio. Stream copy preserves the original bitrate; lossless formats manage compression differently."],
    ],
    note: "Choose Encode to change bitrate when Automatic would copy compatible audio. Supported rates depend on the codec.",
  },
  sampleRate: {
    title: "Audio sample rate",
    summary: "The sample rate is the number of audio measurements per second. Hertz (Hz) means measurements per second; 48,000 Hz is also written as 48 kHz.",
    items: [
      ["Keep source", "Avoids unnecessary resampling and is a good default unless another application requires a specific rate."],
      ["44,100 and 48,000 Hz", "Common choices for music and video workflows, respectively. Use the rate required by your destination."],
      ["Higher is not a repair", "Increasing sample rate does not recover missing frequencies. Some codecs, including Opus, support only particular input rates."],
    ],
    note: "Changing sample rate requires encoding. It does not change playback speed.",
  },
  channels: {
    title: "Audio channels",
    summary: "Channels are separate audio signals, such as left and right speakers.",
    items: [
      ["Keep source", "Retains the source channel layout when supported by the target codec."],
      ["Mono", "Combines the signal into one channel, often useful for speech."],
      ["Stereo", "Uses left and right channels. Surround sources are mixed down; mono sources do not gain new spatial detail."],
    ],
    note: "Changing channels requires encoding and can change the mix you hear.",
  },
  loudness: {
    title: "Loudness normalization",
    summary: "Normalization adjusts perceived listening level. Defaults are −16 LUFS and a −1.5 dBTP true-peak limit; both can be changed in Advanced settings.",
    items: [
      ["LUFS", "A measure of perceived loudness over time. A less negative value is louder."],
      ["dBTP", "A measure of estimated peaks between audio samples. The true-peak limit leaves headroom to reduce clipping."],
      ["Single pass", "Processes the file once. It may alter dynamics and is not a guarantee that every source will finish at exactly the target loudness."],
        ["Measure loudness first", "Analyzes the selected track and time range, then uses those measurements in a second pass. Silence falls back to a single pass. Targets that cannot be reached linearly can still change dynamics."],
    ],
    note: "Normalization requires audio encoding. Leave it off to retain the source level; it is not a noise-removal tool.",
  },
  metadata: {
    title: "Remove metadata",
    summary: "Metadata is descriptive information stored alongside media, such as a title, artist, album, or comment.",
    items: [
      ["On", "Disables copying source metadata into the output. The encoder may still write technical information required by the output format."],
      ["Off", "Allows supported metadata to be copied. The target format may not support every source field."],
      ["What stays", "The source file is unchanged. This toggle does not control subtitles, attachments, or filename text."],
    ],
    note: "Technical format and encoder fields may remain in the converted file.",
  },
} as const;

export type ConversionHelpTopic = keyof typeof HELP;

function HelpDialog({ topic, onClose }: { topic: ConversionHelpTopic; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const summary = useId();
  const help = HELP[topic];
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    const frame = requestAnimationFrame(() => element.classList.add("is-open"));
    return () => {
      cancelAnimationFrame(frame);
      element.close();
      if (previous?.isConnected) previous.focus();
    };
  }, []);
  return <dialog ref={dialog} className="conversion-help-dialog t-modal" aria-labelledby={title} aria-describedby={summary}
    onCancel={(event) => { event.preventDefault(); onClose(); }}
    onClick={(event) => {
      if (event.target !== event.currentTarget) return;
      const rect = event.currentTarget.getBoundingClientRect();
      if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose();
    }}>
    <header className="flex items-start justify-between gap-3 border-b border-edge px-5 py-4">
      <h2 id={title} className="text-[17px] font-medium text-ink">{help.title}</h2>
      <button type="button" aria-label="Close conversion help" onClick={onClose} className="entry-icon-button focus-ring -mr-1 -mt-1"><span className="entry-icon-glyph"><Close className="size-4" /></span></button>
    </header>
    <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4 text-[13px] leading-relaxed text-ink-dim">
      <p id={summary}>{help.summary}</p>
      <dl className="mt-4 space-y-3">{help.items.map(([term, explanation]) => <div key={term}><dt className="font-medium text-ink">{term}</dt><dd className="mt-1">{explanation}</dd></div>)}</dl>
      <p className="mt-4 border-t border-edge pt-3 text-[12px] text-ink-faint">{help.note}</p>
    </div>
    <footer className="flex justify-end border-t border-edge px-5 py-3"><Button tone="ghost" onClick={onClose}>Got it</Button></footer>
  </dialog>;
}

export function ConversionHelp({ topic }: { topic: ConversionHelpTopic }) {
  const [open, setOpen] = useState(false);
  const [tooltipSuppressed, setTooltipSuppressed] = useState(false);
  return <>
    <TooltipGroup className="inline-flex shrink-0" side="top" disabled={open || tooltipSuppressed}>
      <button type="button" aria-label={`About ${HELP[topic].title}`} aria-haspopup="dialog" data-tooltip={`About ${HELP[topic].title}`}
        onClick={() => { setTooltipSuppressed(true); setOpen(true); }}
        onPointerLeave={() => { if (!open) setTooltipSuppressed(false); }}
        onBlur={() => { if (!open) setTooltipSuppressed(false); }}
        className="t-tt-trigger focus-ring inline-flex size-6 items-center justify-center rounded-full text-ink-faint hover:bg-route/10 hover:text-route"><Info className="size-3.5" /></button>
    </TooltipGroup>
    {open && <HelpDialog topic={topic} onClose={() => setOpen(false)} />}
  </>;
}
