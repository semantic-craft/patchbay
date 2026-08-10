/**
 * 实体标识色（K4 彩标）。项目与技能各自按名字取一色，同一个名字在任何界面、
 * 任何会话都是同一色——颜色是身份标签，不承载状态语义（状态另有绿/琥珀）。
 *
 * 六个色相在色轮上尽量分散，中间调，配 12% 同色底，浅色与深色画布上都可读。
 */
const PALETTE = [
  { color: "#0891B2", background: "rgba(8,145,178,0.12)" },
  { color: "#D97706", background: "rgba(217,119,6,0.12)" },
  { color: "#7C3AED", background: "rgba(124,58,237,0.12)" },
  { color: "#DB2777", background: "rgba(219,39,119,0.12)" },
  { color: "#2563EB", background: "rgba(37,99,235,0.12)" },
  { color: "#16A34A", background: "rgba(22,163,74,0.12)" },
] as const;

export interface EntityColor {
  color: string;
  background: string;
}

/** Stable name → colour. Plain FNV-style rolling hash; identical names collide
 *  with each other and nothing else, which is exactly the intent. */
export function entityColor(name: string): EntityColor {
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) | 0;
  return PALETTE[Math.abs(hash) % PALETTE.length];
}
