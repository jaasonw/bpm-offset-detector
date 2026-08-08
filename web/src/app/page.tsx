import Analyzer from "@/components/Analyzer";

export default function Home() {
  return (
    <div className="flex flex-1 flex-col items-center bg-zinc-50 px-6 py-16 font-sans dark:bg-black">
      <main className="flex w-full max-w-2xl flex-col items-center gap-8">
        <div className="flex flex-col items-center gap-2 text-center">
          <h1 className="text-2xl font-semibold tracking-tight text-zinc-900 dark:text-zinc-50">
            BPM &amp; Offset Detector
          </h1>
          <p className="max-w-md text-sm text-zinc-600 dark:text-zinc-400">
            Upload an audio file to detect bpm and offset. Analysis runs entirely in your
            browser.
          </p>
        </div>
        <Analyzer />
      </main>
      <footer className="mt-16 flex flex-col items-center gap-1 text-xs text-zinc-500 dark:text-zinc-500">
        <p>
          Copyright 2025 &mdash; open source under the{" "}
          <a
            href="https://github.com/jaasonw/bpm-offset-detector/blob/main/LICENSE"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-zinc-700 dark:hover:text-zinc-300"
          >
            GPL-3.0 license
          </a>
        </p>
        <p className="flex gap-3">
          <a
            href="https://github.com/jaasonw/bpm-offset-detector"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-zinc-700 dark:hover:text-zinc-300"
          >
            View source on GitHub
          </a>
          <span aria-hidden>&middot;</span>
          <a
            href="https://ko-fi.com/wayson"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-zinc-700 dark:hover:text-zinc-300"
          >
            Donate
          </a>
        </p>
      </footer>
    </div>
  );
}
