import { useEffect, useState } from "react";
import type { TicketInfo } from "@contracts/ipc";
import { api, errorMessage } from "../lib/ipc";
import { useStore } from "../lib/store";
import { Icon } from "./ui";

/** Shows a created ticket as: web link + QR + copy buttons. */
export function QrLink({ info, caption }: { info: TicketInfo; caption?: string }) {
  const [svg, setSvg] = useState<string>("");
  const [copied, setCopied] = useState<string | null>(null);
  const toast = useStore((s) => s.toast);

  useEffect(() => {
    let alive = true;
    api.qr_svg({ text: info.web_url }).then((s) => alive && setSvg(s)).catch((e) => toast(errorMessage(e), "error"));
    return () => { alive = false; };
  }, [info.web_url, toast]);

  async function copy(what: string, text: string) {
    try {
      await api.copy_text({ text });
      setCopied(what);
      setTimeout(() => setCopied((c) => (c === what ? null : c)), 1800);
    } catch (e) {
      toast(errorMessage(e), "error");
    }
  }

  return (
    <div className="rounded-[16px] hair bg-line/[0.04] p-3 rise">
      {caption && <div className="text-xs text-mute mb-2">{caption}</div>}
      <div className="flex gap-3 items-start">
        <div className="w-[104px] h-[104px] shrink-0 rounded-[12px] bg-white p-1.5 overflow-hidden shadow-[0_8px_24px_-12px_rgb(var(--accent)/0.6)]" dangerouslySetInnerHTML={{ __html: svg }} aria-label="Mã QR" />
        <div className="min-w-0 flex-1">
          <div className="selectable text-xs mono break-all line-clamp-4 text-ink" title={info.web_url}>{info.web_url}</div>
          <div className="flex flex-wrap gap-2 mt-2">
            <button className="btn btn-primary" onClick={() => copy("web", info.web_url)}>
              <Icon name={copied === "web" ? "check" : "copy"} weight="bold" />{copied === "web" ? "Đã chép" : "Sao chép link"}
            </button>
            <button className="btn btn-ghost" onClick={() => copy("raw", info.ticket)} title="Chép mã toss1… (dán thẳng vào app)">
              <Icon name={copied === "raw" ? "check" : "link"} />{copied === "raw" ? "Đã chép" : "Mã"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
