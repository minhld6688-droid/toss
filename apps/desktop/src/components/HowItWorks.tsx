// The one place that explains how Toss works. Every screen that needs the explanation renders this
// (Home: "full", empty states and the setup screen: "line"), so the wording never drifts apart.
import { useState } from "react";
import { Hint } from "./Hint";
import { Icon } from "./ui";

export const HOW_LINE = "Mở Toss hoặc LocalSend trên máy kia (cùng wifi) là thấy nhau ở đây. Kéo file thả vào tên máy là gửi.";

const HOW_STEPS: { title: string; text: string }[] = [
  { title: "Cùng mạng", text: "Mở Toss hoặc LocalSend trên máy kia, tên máy hiện ngay ở “Gần đây”. Kéo file thả vào tên là gửi." },
  { title: "Máy ở xa", text: "Bấm “Mời máy ở xa” một lần. Từ đó hai máy nằm trong Danh bạ và gửi cho nhau được dù ở đâu." },
  { title: "Người khác", text: "Tạo link cho một file hoặc folder, hoặc “Chia sẻ qua trình duyệt” cho máy không cài app." },
];

export function HowItWorks({ variant = "line", className = "" }: { variant?: "line" | "full"; className?: string }) {
  if (variant === "line") return <Hint className={className}>{HOW_LINE}</Hint>;
  return (
    <ol className={`card px-3.5 py-3 space-y-2.5 text-sm ${className}`}>
      {HOW_STEPS.map((s, i) => (
        <li key={s.title} className="flex gap-2.5">
          <span className="mono text-accent font-semibold">{i + 1}</span>
          <span><b className="font-medium">{s.title}:</b> {s.text}</span>
        </li>
      ))}
      <li><Hint>File nhận về nằm trong thư mục nhận (Cài đặt → Máy này). Giữa hai máy Toss, file đi thẳng, không qua máy chủ.</Hint></li>
    </ol>
  );
}

/** "Toss dùng thế nào?": closed by default, same wording as everywhere else. */
export function HowItWorksFold({ className = "" }: { className?: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={className}>
      <button className="h-section !mb-2 inline-flex items-center gap-1.5 hover:text-ink transition-colors" onClick={() => setOpen(!open)} aria-expanded={open}>
        <Icon name="info" className="w-3.5 h-3.5" />Toss dùng thế nào?
        <Icon name="chevron" className={`w-3.5 h-3.5 transition-transform duration-500 ease-aura ${open ? "rotate-180" : ""}`} />
      </button>
      {open && <HowItWorks variant="full" />}
    </div>
  );
}
