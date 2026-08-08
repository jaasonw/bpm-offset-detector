import Analyzer from "@/components/Analyzer";
import { ThemeToggle } from "@/components/theme-toggle";

export default function Home() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center bg-background px-6 py-6 font-sans">
      <main className="flex w-full max-w-2xl flex-col items-center gap-3">
        <div className="flex flex-col items-center gap-1 text-center">
          <h1 className="text-2xl font-semibold tracking-tight text-foreground">
            BPM &amp; Offset Detector
          </h1>
          <p className="max-w-none text-sm text-muted-foreground sm:whitespace-nowrap">
            Upload an audio file to detect bpm and offset. Analysis runs entirely in your
            browser.
          </p>
        </div>
        <Analyzer />
      </main>
      <footer className="mt-6 flex flex-col items-center gap-1 text-xs text-muted-foreground">
        <p>
          Algorithm by{" "}
          <a
            href="https://github.com/jaasonw/bpm-offset-detector/blob/main/original-paper/report.pdf"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-foreground"
          >
            Bram van de Wetering
          </a>
          , based on{" "}
          <a
            href="https://github.com/jaasonw/bpm-offset-detector/blob/main/original/doc/syslab-version/paper.pdf"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-foreground"
          >
            Nathan Stephenson&apos;s implementation
          </a>
        </p>
        <p>
          Copyright 2026 &mdash; open source under{" "}
          <a
            href="https://github.com/jaasonw/bpm-offset-detector/blob/main/LICENSE"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-foreground"
          >
            GPLv3
          </a>
        </p>
        <p className="flex items-center gap-3">
          <a
            href="https://github.com/jaasonw/bpm-offset-detector"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-foreground"
          >
            View source on GitHub
          </a>
          <span aria-hidden>&middot;</span>
          <a
            href="https://ko-fi.com/wayson"
            target="_blank"
            rel="noopener noreferrer"
            className="underline hover:text-foreground"
          >
            Donate
          </a>
          <span aria-hidden>&middot;</span>
          <ThemeToggle />
        </p>
      </footer>
    </div>
  );
}
