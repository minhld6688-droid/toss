import type { ReactNode } from "react";
import { Icon } from "./ui";

/** One plain-language line under a control: what it does, when you need it (v0.6). */
export function Hint({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <div className={`text-xs text-mute flex gap-1.5 leading-relaxed ${className}`}>
      <Icon name="info" className="w-3.5 h-3.5 mt-[3px] shrink-0" />
      <span>{children}</span>
    </div>
  );
}
