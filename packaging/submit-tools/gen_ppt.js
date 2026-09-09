// 麟忆尽智 lymem · 项目报告 PPT 生成脚本(pptxgenjs)
// 用法: NODE_PATH=$(npm root -g) node gen_ppt.js  → 项目报告-lymem.pptx
const pptxgen = require("pptxgenjs");
// 产物输出到 lymem/dist/submit(素材在 packaging/submit-tools/assets)
const path = require("path");
const A = path.join(__dirname, "assets");
process.chdir(path.join(__dirname, "..", "..", "dist", "submit"));

const p = new pptxgen();
p.layout = "LAYOUT_WIDE"; // 13.33 × 7.5"
p.author = "TJUEZ";
p.title = "麟忆尽智 lymem · OS Agent 记忆优化及高效应用";

// ---- 配色(取自产品本体:藏青侧栏 / 蓝色主色 / 琥珀警示) ----
const DARK = "152238", PRIMARY = "1E3A5F", ACCENT = "2563EB";
const TEXT = "1E293B", MUTED = "64748B", TINT = "EEF3FB", LINE = "D8E2F0";
const OK = "15803D", AMBER = "B45309", LIGHT = "A8C0E8";
const F = "微软雅黑";
const W = 13.33, H = 7.5, M = 0.5;

const bu = () => ({ code: "25B8", indent: 12 });
const shadow = () => ({ type: "outer", color: "1E3A5F", blur: 7, offset: 2, angle: 45, opacity: 0.18 });

function pageChrome(s, n, section) {
  s.background = { color: "FFFFFF" };
  s.addText(`${section} ｜ 麟忆尽智 lymem`, { x: M, y: 7.12, w: 6, h: 0.3, fontSize: 10.5, fontFace: F, color: MUTED, margin: 0 });
  s.addText(String(n).padStart(2, "0"), { x: W - 1.0, y: 7.12, w: 0.5, h: 0.3, fontSize: 10.5, fontFace: F, color: MUTED, align: "right", margin: 0 });
}
function slideTitle(s, kicker, title) {
  s.addText(kicker, { x: M, y: 0.32, w: 9, h: 0.3, fontSize: 13, fontFace: F, color: ACCENT, bold: true, margin: 0 });
  s.addText(title, { x: M, y: 0.62, w: W - 2 * M, h: 0.62, fontSize: 29, fontFace: F, color: TEXT, bold: true, margin: 0 });
}
function card(s, x, y, w, h, fill = "FFFFFF") {
  s.addShape(p.shapes.ROUNDED_RECTANGLE, { x, y, w, h, fill: { color: fill }, line: { color: LINE, width: 1 }, rectRadius: 0.07, shadow: shadow() });
}
function stat(s, x, y, w, num, label, color = ACCENT, numSize = 40) {
  s.addText(num, { x, y, w, h: 0.75, fontSize: numSize, fontFace: F, color, bold: true, align: "center", margin: 0 });
  s.addText(label, { x, y: y + 0.78, w, h: 0.55, fontSize: 12.5, fontFace: F, color: MUTED, align: "center", margin: 0 });
}
function pic(s, path, x, y, h, cap) {
  const w = h * (1440 / 900);
  s.addShape(p.shapes.ROUNDED_RECTANGLE, { x: x - 0.06, y: y - 0.06, w: w + 0.12, h: h + 0.12, fill: { color: "FFFFFF" }, line: { color: LINE, width: 1 }, rectRadius: 0.06, shadow: shadow() });
  s.addImage({ path, x, y, w, h });
  if (cap) s.addText(cap, { x, y: y + h + 0.08, w, h: 0.3, fontSize: 11.5, fontFace: F, color: MUTED, align: "center", margin: 0 });
  return w;
}
function srcNote(s, txt, y = 6.72) {
  s.addText(txt, { x: M, y, w: W - 2 * M, h: 0.3, fontSize: 10.5, fontFace: F, color: MUTED, margin: 0 });
}

// ================= S1 封面 =================
{
  const s = p.addSlide();
  s.background = { color: DARK };
  s.addText("挑战杯「揭榜挂帅」擂台赛 · 选题 XA-202612 · OS Agent 记忆优化及高效应用研究", { x: M, y: 0.9, w: W - 2 * M, h: 0.35, fontSize: 14, fontFace: F, color: LIGHT, margin: 0 });
  s.addText("麟忆尽智 lymem", { x: M, y: 2.0, w: W - 2 * M, h: 1.3, fontSize: 60, fontFace: F, color: "FFFFFF", bold: true, margin: 0 });
  s.addText("面向银河麒麟桌面的 OS Agent 端侧记忆系统", { x: M, y: 3.35, w: W - 2 * M, h: 0.55, fontSize: 24, fontFace: F, color: LIGHT, margin: 0 });
  s.addText("多源融合灌入 · 四层记忆流转 · 冲突仲裁 · 经验复用 · 端侧轻量化", { x: M, y: 3.95, w: W - 2 * M, h: 0.4, fontSize: 15, fontFace: F, color: "7E96BC", margin: 0 });
  const stats = [["98.0%", "偏好提取准确率"], ["16.5ms", "检索延迟 p50"], ["+6.7pts", "Agent 任务提升"], ["100%", "功能离线可用"]];
  stats.forEach((st, i) => {
    const x = M + i * 3.15;
    s.addText(st[0], { x, y: 5.0, w: 3.0, h: 0.7, fontSize: 36, fontFace: F, color: i === 2 ? "FFD479" : "FFFFFF", bold: true, margin: 0 });
    s.addText(st[1], { x, y: 5.7, w: 3.0, h: 0.35, fontSize: 12.5, fontFace: F, color: LIGHT, margin: 0 });
  });
  s.addText("提报单位：（学校全称）    团队成员：（待填写）    指导教师：（待填写）", { x: M, y: 6.75, w: W - 2 * M, h: 0.35, fontSize: 12, fontFace: F, color: "7E96BC", margin: 0 });
}

// ================= S2 背景与痛点 =================
{
  const s = p.addSlide();
  pageChrome(s, 2, "背景与问题");
  slideTitle(s, "背景与问题", "OS Agent 的记忆,为什么难用?");
  const pains = [
    ["工具结果结构化不足", "数据质量参差不齐 → 偏好提取不精准、知识沉淀低效"],
    ["用户行为捕捉不全", "跨场景数据不一致 → 动态偏好适配失效"],
    ["配置版本混乱", "偏好缺少版本化管理 → 回溯与切换困难"],
    ["新旧知识冲突滞后", "矛盾知识并存 → 关联检索精度受限"],
  ];
  pains.forEach((pi, i) => {
    const y = 1.5 + i * 1.12;
    s.addShape(p.shapes.ROUNDED_RECTANGLE, { x: M, y, w: 0.62, h: 0.62, fill: { color: TINT }, line: { color: LINE, width: 1 }, rectRadius: 0.1 });
    s.addText(String(i + 1), { x: M, y, w: 0.62, h: 0.62, fontSize: 22, fontFace: F, color: ACCENT, bold: true, align: "center", valign: "middle", margin: 0 });
    s.addText([
      { text: pi[0], options: { fontSize: 17, bold: true, color: TEXT, breakLine: true } },
      { text: pi[1], options: { fontSize: 13, color: MUTED } },
    ], { x: M + 0.85, y: y - 0.08, w: 5.3, h: 1.0, fontFace: F, valign: "top", margin: 0 });
  });
  pic(s, A + "/dash.png", 6.9, 1.45, 3.7, "lymem 管理台:总览页(演示数据一键灌入)");
  s.addShape(p.shapes.RECTANGLE, { x: M, y: 6.15, w: 12.33, h: 0.72, fill: { color: TINT } });
  s.addText([
    { text: "方案定位  ", options: { bold: true, color: ACCENT } },
    { text: "纯 Rust 实现的端侧长期记忆系统:多源融合灌入、四层流转、冲突仲裁、经验复用。全部数据留在本机,检索延迟是 500ms 预算的 3%。", options: { color: TEXT } },
  ], { x: M + 0.2, y: 6.15, w: 12.0, h: 0.72, fontSize: 14.5, fontFace: F, valign: "middle", margin: 0 });
}

// ================= S3 系统架构 =================
{
  const s = p.addSlide();
  pageChrome(s, 3, "方案总览");
  slideTitle(s, "方案总览", "系统架构:单进程、单文件、全端侧");
  const layers = [
    ["接入层", "REST API(/api/v1/*)  ·  MCP 8 个记忆工具  ·  /v1/embeddings OpenAI 兼容代理  ·  Web 管理台(中文 7 页)", PRIMARY],
    ["记忆核心 lymem-core", "四层记忆(工作/情景/知识/偏好)  ｜  管道:清洗·敏感分级·冲突四步·蒸馏·遗忘  ｜  RRF 三路混合检索", ACCENT],
    ["端侧适配", "lymem-kylin:麒麟嵌入 SDK(DBus→kytensor→hash 三级自愈)  ｜  lymem-llm:MiniMax 兼容网关(无 Key 规则兜底)", PRIMARY],
    ["存储", "SQLite(WAL) + sqlite-vec(向量) + tantivy(全文),单文件库,备份就是拷贝", PRIMARY],
  ];
  layers.forEach((L, i) => {
    const y = 1.55 + i * 1.18;
    card(s, M, y, 9.6, 1.0, i === 1 ? TINT : "FFFFFF");
    s.addText(L[0], { x: M + 0.25, y: y + 0.12, w: 2.6, h: 0.75, fontSize: 16, fontFace: F, color: L[2], bold: true, valign: "middle", margin: 0 });
    s.addText(L[1], { x: M + 2.95, y: y + 0.12, w: 6.5, h: 0.75, fontSize: 12.5, fontFace: F, color: TEXT, valign: "middle", margin: 0 });
  });
  s.addText("纯 Rust", { x: 10.4, y: 1.55, w: 2.4, h: 0.9, fontSize: 30, fontFace: F, color: ACCENT, bold: true, align: "center", valign: "middle", margin: 0 });
  s.addText("单二进制 19.7 MB\n动态依赖仅 glibc 6 项\nSQLite / sqlite-vec / tantivy 全静态链接\n零第三方 .so", { x: 10.4, y: 2.6, w: 2.4, h: 2.6, fontSize: 13, fontFace: F, color: TEXT, align: "center", margin: 0 });
  s.addText("Workspace:lymem-core / kylin / llm / server / cli", { x: 10.4, y: 5.3, w: 2.4, h: 1.0, fontSize: 11.5, fontFace: F, color: MUTED, align: "center", margin: 0 });
  s.addText("服务仅绑定 127.0.0.1;LLM 为可选外呼,不配置 Key 即完全离线运行。", { x: M, y: 6.4, w: 12.33, h: 0.35, fontSize: 12.5, fontFace: F, color: MUTED, margin: 0 });
}

// ================= S4 四层记忆与流转 =================
{
  const s = p.addSlide();
  pageChrome(s, 4, "记忆流转(条款 6)");
  slideTitle(s, "条款 6 · 记忆流转", "四层记忆体系:短期 → 中期 → 长期");
  const tiers = [
    ["工作记忆 · 短期", "会话内即时状态,自动 flush", "152238"],
    ["情景记忆 · 中期", "会话轮次与工具轨迹(成败标注)", PRIMARY],
    ["知识记忆 · 长期", "工作流 / 案例 / 模板,版本化", ACCENT],
    ["偏好记忆 · 长期", "双通道提取,版本链回溯", ACCENT],
  ];
  tiers.forEach((t, i) => {
    const y = 1.5 + i * 1.32;
    s.addShape(p.shapes.ROUNDED_RECTANGLE, { x: M, y, w: 4.4, h: 0.95, fill: { color: t[2] }, rectRadius: 0.08, shadow: shadow() });
    s.addText([
      { text: t[0], options: { fontSize: 16, bold: true, color: "FFFFFF", breakLine: true } },
      { text: t[1], options: { fontSize: 11.5, color: "C9D8F0" } },
    ], { x: M + 0.25, y: y + 0.08, w: 3.9, h: 0.8, fontFace: F, valign: "middle", margin: 0 });
    if (i < 3) s.addText(i === 0 ? "flush ▼" : i === 1 ? "consolidate(蒸馏)▼" : "提取 ▲", { x: M + 4.55, y: y + 0.25, w: 1.6, h: 0.5, fontSize: 12, fontFace: F, color: MUTED, margin: 0 });
  });
  const notes = [
    ["写入即分级", "四类事件按语义入库到对应层,清洗/敏感分级/去重先行"],
    ["正向流转", "蒸馏把情景记忆沉淀为知识;规则挖掘与 LLM 提取把轨迹沉淀为偏好"],
    ["反向可用", "检索跨层联合召回;偏好按场景过滤生效,任意时点回溯版本"],
    ["可删可审计", "自然语言精准遗忘(墓碑软删/硬清除),全部操作留审计痕"],
  ];
  notes.forEach((n, i) => {
    const y = 1.5 + i * 1.28;
    card(s, 6.9, y, 5.9, 1.1);
    s.addText([
      { text: n[0], options: { fontSize: 15, bold: true, color: ACCENT, breakLine: true } },
      { text: n[1], options: { fontSize: 12.5, color: TEXT } },
    ], { x: 7.15, y: y + 0.1, w: 5.4, h: 0.9, fontFace: F, valign: "middle", margin: 0 });
  });
  srcNote(s, "与短期/中期记忆的交互:会话内存放工作层,会话结束 flush 入情景层;蒸馏/提取将中期沉淀为长期,检索时四层联合。");
}

// ================= S5 条款1 多源整合 =================
{
  const s = p.addSlide();
  pageChrome(s, 5, "多源整合(条款 1)");
  slideTitle(s, "条款 1 · 多源数据整合", "四类数据源,一条清洗管道");
  const srcs = [["工具执行结果", "tool / task / ok / output,赛题要求的核心数据源"], ["用户行为数据", "跨场景行为事件(应用使用/操作习惯)"], ["手动配置信息", "键值配置,直接成偏好(置信度 1.0)"], ["会话轮次", "对话陈述,供 LLM 偏好提取通道"]];
  srcs.forEach((c, i) => {
    const y = 1.5 + i * 1.18;
    card(s, M, y, 5.4, 1.0);
    s.addText([
      { text: c[0], options: { fontSize: 15.5, bold: true, color: TEXT, breakLine: true } },
      { text: c[1], options: { fontSize: 12, color: MUTED } },
    ], { x: M + 0.25, y: y + 0.08, w: 4.9, h: 0.85, fontFace: F, valign: "middle", margin: 0 });
  });
  const steps = ["统一接入", "清洗规范化", "敏感分级", "去重指纹", "实体抽取", "分层入库"];
  steps.forEach((st, i) => {
    const y = 1.6 + i * 0.78;
    s.addShape(p.shapes.ROUNDED_RECTANGLE, { x: 6.6, y, w: 3.1, h: 0.62, fill: { color: i === 5 ? ACCENT : TINT }, rectRadius: 0.08 });
    s.addText(st, { x: 6.6, y, w: 3.1, h: 0.62, fontSize: 14.5, fontFace: F, color: i === 5 ? "FFFFFF" : TEXT, bold: i === 5, align: "center", valign: "middle", margin: 0 });
    if (i < 5) s.addText("▼", { x: 8.0, y: y + 0.56, w: 0.3, h: 0.3, fontSize: 11, fontFace: F, color: MUTED, margin: 0 });
  });
  s.addText([
    { text: "质量校验报告(逐事件预检不入库):", options: { bold: true, color: TEXT, breakLine: true } },
    { text: "清洗前后长度 / 命中敏感级 / 实体 / 重复判定 / 指纹,界面可视化;被拦截内容不留痕入库,仅记审计。", options: { color: MUTED } },
  ], { x: 10.0, y: 1.7, w: 2.9, h: 4.4, fontSize: 12.5, fontFace: F, margin: 0 });
  srcNote(s, "对应管理台「记忆库」页:粘贴文本 / 上传文档 / 事件 JSON 三种灌入方式;「演示数据」一键灌入四源样例。");
}

// ================= S6 条款2 偏好记忆 =================
{
  const s = p.addSlide();
  pageChrome(s, 6, "偏好记忆(条款 2)");
  slideTitle(s, "条款 2 · 偏好动态捕捉", "双通道提取 + 版本链,98.0% 提取准确率");
  card(s, M, 1.5, 6.3, 2.35);
  s.addText("规则快通道", { x: M + 0.25, y: 1.7, w: 2.6, h: 0.4, fontSize: 15, fontFace: F, color: TEXT, bold: true, margin: 0 });
  s.addText("工具使用统计即时挖掘,无 LLM 可用", { x: M + 0.25, y: 2.1, w: 2.6, h: 0.8, fontSize: 12, fontFace: F, color: MUTED, margin: 0 });
  card(s, M + 3.35, 1.5, 2.7, 2.35, TINT);
  s.addText("LLM 慢通道", { x: M + 3.6, y: 1.7, w: 2.3, h: 0.4, fontSize: 15, fontFace: F, color: TEXT, bold: true, margin: 0 });
  s.addText("会话陈述分析提取(如\"以后都用 PDF\")", { x: M + 3.6, y: 2.1, w: 2.3, h: 0.9, fontSize: 12, fontFace: F, color: MUTED, margin: 0 });
  s.addText("→", { x: M + 2.9, y: 2.4, w: 0.5, h: 0.5, fontSize: 22, fontFace: F, color: ACCENT, bold: true, align: "center", margin: 0 });
  card(s, M, 4.1, 6.3, 1.9);
  s.addText([
    { text: "版本链 + 跨场景适配", options: { fontSize: 15, bold: true, color: TEXT, breakLine: true } },
    { text: "同键重复写入追加版本并关闭旧版本;按场景过滤生效(如\"工作\"与\"生活\"不同偏好);任意时点回溯历史版本。", options: { fontSize: 12.5, color: MUTED } },
  ], { x: M + 0.25, y: 4.3, w: 5.8, h: 1.5, fontFace: F, margin: 0 });
  s.addChart(p.charts.BAR, [{ name: "PrefEval 存储协议(100 题)", labels: ["lymem", "mem0", "iwe"], values: [98.0, 73.0, 53.0] }], {
    x: 7.2, y: 1.5, w: 5.6, h: 3.6, barDir: "col", varyColors: true,
    chartColors: [ACCENT, "9AB2D6", "C9D4E6"],
    chartArea: { fill: { color: "FFFFFF" } },
    catAxisLabelColor: MUTED, valAxisLabelColor: MUTED, catAxisLabelFontFace: F, valAxisLabelFontFace: F, dataLabelFontFace: F,
    valAxisMaxVal: 100, valGridLine: { color: "E2E8F0", size: 0.5 }, catGridLine: { style: "none" },
    showValue: true, dataLabelPosition: "outEnd", dataLabelColor: TEXT, showLegend: false,
    showTitle: true, title: "偏好提取准确率(%) · 目标 ≥85%", titleFontFace: F, titleFontSize: 13, titleColor: TEXT,
  });
  s.addText("会话偏好提取专项 100%(近期真实会话流)", { x: 7.2, y: 5.25, w: 5.6, h: 0.35, fontSize: 13, fontFace: F, color: OK, bold: true, align: "center", margin: 0 });
  srcNote(s, "数据来源:benchmark/results/REPORT.md §五(PrefEval 存储协议 r8,对齐必答 MCQ);对比系统接入同一麒麟嵌入与生成网关的公平协议。");
}

// ================= S7 条款3 知识与冲突 =================
{
  const s = p.addSlide();
  pageChrome(s, 7, "知识整合(条款 3)");
  slideTitle(s, "条款 3 · 知识结构化整合", "冲突四步管道:写入即仲裁,93.3% 正确率");
  const steps = [["检测", "实体门 + 相似度(0.8)召回旧知识"], ["分类", "数值更新 / 方法过时 / 前提失效 / 表面相似 / 取值矛盾"], ["仲裁", "保留新 / 保留旧 / 共存 / 合并(LLM 复核,规则兜底)"], ["版本化", "版本链 + 冲突台账,全程可回溯"]];
  steps.forEach((st, i) => {
    const x = M + i * 3.13;
    card(s, x, 1.5, 2.85, 1.75, i === 3 ? TINT : "FFFFFF");
    s.addText(st[0], { x, y: 1.66, w: 2.85, h: 0.45, fontSize: 17, fontFace: F, color: ACCENT, bold: true, align: "center", margin: 0 });
    s.addText(st[1], { x: x + 0.2, y: 2.14, w: 2.45, h: 1.0, fontSize: 11.5, fontFace: F, color: TEXT, align: "center", margin: 0 });
    if (i < 3) s.addText("→", { x: x + 2.82, y: 2.1, w: 0.35, h: 0.5, fontSize: 20, fontFace: F, color: MUTED, align: "center", margin: 0 });
  });
  card(s, M, 3.7, 6.0, 2.5);
  s.addText("93.3%", { x: M, y: 3.95, w: 6.0, h: 0.95, fontSize: 54, fontFace: F, color: OK, bold: true, align: "center", margin: 0 });
  s.addText("冲突处理正确率(30 组受控用例,28/30)· 目标 ≥88%", { x: M, y: 4.95, w: 6.0, h: 0.4, fontSize: 13, fontFace: F, color: MUTED, align: "center", margin: 0 });
  s.addText("另:MAB-conflict 子集 EM 18.0%,为对比系统唯一非零(词法/抽取式基线均为 0)", { x: M + 0.3, y: 5.5, w: 5.4, h: 0.6, fontSize: 12, fontFace: F, color: TEXT, align: "center", margin: 0 });
  card(s, 6.85, 3.7, 5.95, 2.5);
  s.addText([
    { text: "工作流 / 案例 / 模板 结构化存储", options: { fontSize: 14.5, bold: true, color: TEXT, breakLine: true } },
    { text: "kind 标签(工作流/案例/模板/事实)+ 实体图边,支撑关联检索与智能调用", options: { fontSize: 12, color: MUTED, breakLine: true } },
    { text: "跨源矛盾扫描", options: { fontSize: 14.5, bold: true, color: TEXT, breakLine: true } },
    { text: "多个终端 Agent 汇报的同一事实不一致时按实体聚合检测,LLM 复核后仲裁,与知识冲突共用版本链", options: { fontSize: 12, color: MUTED } },
  ], { x: 7.1, y: 3.9, w: 5.45, h: 2.2, fontFace: F, paraSpaceAfter: 6, margin: 0 });
  srcNote(s, "数据来源:REPORT.md §四、§五;conflict_bench 受控用例与 MAB-conflict 采样评测,逐题明细落盘 results/。");
}

// ================= S8 条款4 端侧部署 =================
{
  const s = p.addSlide();
  pageChrome(s, 8, "端侧部署(条款 4)");
  slideTitle(s, "条款 4 · 麒麟端侧部署", "调用麒麟嵌入 SDK,延迟预算的 3%");
  const chans = [["① DBus SDK 直连", "麒麟 AI runtime 用户级套接字,768 维 gte 嵌入(首选)"], ["② kytensor 直连", "DBus 退化时自动切本机 Triton HTTP(127.0.0.1:8000)"], ["③ 本地哈希兜底", "完全离线降级,功能不中断"]];
  chans.forEach((c, i) => {
    const y = 1.5 + i * 1.15;
    card(s, M, y, 6.1, 0.98, i === 0 ? TINT : "FFFFFF");
    s.addText([
      { text: c[0] + "   ", options: { fontSize: 14.5, bold: true, color: i === 0 ? ACCENT : TEXT } },
      { text: c[1], options: { fontSize: 12, color: MUTED } },
    ], { x: M + 0.25, y: y + 0.08, w: 5.6, h: 0.82, fontFace: F, valign: "middle", margin: 0 });
  });
  s.addText("嵌入对齐:提供 /v1/embeddings 代理,mem0 等基线复用同一麒麟嵌入模型,保证横向对比公平(对齐复测见报告 §〇一.5)。", { x: M, y: 5.05, w: 6.1, h: 0.85, fontSize: 12.5, fontFace: F, color: TEXT, margin: 0 });
  stat(s, 7.0, 1.5, 2.9, "16.5ms", "检索 p50(预算的 3.0%)", ACCENT, 36);
  stat(s, 9.95, 1.5, 2.9, "29.4ms", "检索 p95(预算的 5.9%)", ACCENT, 36);
  stat(s, 7.0, 3.35, 2.9, "~255ms", "启动到端口就绪", ACCENT, 36);
  stat(s, 9.95, 3.35, 2.9, "0/442", "超标查询数(暖态全量)", ACCENT, 36);
  s.addText([
    { text: "交付形态:", options: { bold: true, color: TEXT } },
    { text: "deb 包 5.5MB · 免 root 便携包 7.6MB · 应用菜单 + 登录自启 + lymem 六命令;单文件 SQLite,备份即整目录拷贝。", options: { color: MUTED } },
  ], { x: 7.0, y: 5.15, w: 5.85, h: 0.9, fontSize: 12.5, fontFace: F, margin: 0 });
  srcNote(s, "数据来源:适配测试报告(银河麒麟桌面 V11 真机,x86_64);442 查询 / 3,529 docs 暖态口径,详见 PERF.md。");
}

// ================= S9 条款5 敏感与遗忘 =================
{
  const s = p.addSlide();
  pageChrome(s, 9, "敏感与遗忘(条款 5)");
  slideTitle(s, "条款 5 · 敏感识别与精准遗忘", "自然语言遗忘:前后对比,立刻见效");
  const steps = [["遗忘前检索", "同一问题先查一次,留下基线结果"], ["下达自然语言指令", "如\"忘掉关于部署路径的一切\""], ["解析并预览", "LLM 解析四维范围(主题/场景/层级/时间),无 LLM 规则兜底;确认后执行"], ["遗忘后再检索", "前后对比直观验证;软删可恢复 / 硬清除两档"]];
  steps.forEach((st, i) => {
    const y = 1.5 + i * 1.22;
    s.addShape(p.shapes.OVAL, { x: M, y: y + 0.08, w: 0.58, h: 0.58, fill: { color: i === 3 ? OK : ACCENT } });
    s.addText(String(i + 1), { x: M, y: y + 0.08, w: 0.58, h: 0.58, fontSize: 20, fontFace: F, color: "FFFFFF", bold: true, align: "center", valign: "middle", margin: 0 });
    if (i < 3) s.addShape(p.shapes.LINE, { x: M + 0.29, y: y + 0.7, w: 0, h: 0.62, line: { color: LINE, width: 2 } });
    s.addText([
      { text: st[0], options: { fontSize: 15, bold: true, color: TEXT, breakLine: true } },
      { text: st[1], options: { fontSize: 12, color: MUTED } },
    ], { x: M + 0.85, y: y - 0.02, w: 5.3, h: 1.15, fontFace: F, margin: 0 });
  });
  card(s, 7.0, 1.5, 5.8, 2.5);
  s.addText([
    { text: "敏感信息分级(纯本地规则引擎)", options: { fontSize: 15, bold: true, color: TEXT, breakLine: true } },
    { text: "手机号 / 身份证 / 银行卡 / 密钥 / IP 地址等规则识别;入库前自动脱敏(替换为 [类型:长度]);", options: { fontSize: 12.5, color: MUTED, breakLine: true } },
    { text: "界面提供扫描测试台:命中项标红 + 脱敏改写预览", options: { fontSize: 12.5, color: MUTED } },
  ], { x: 7.25, y: 1.72, w: 5.3, h: 2.1, fontFace: F, paraSpaceAfter: 8, margin: 0 });
  card(s, 7.0, 4.25, 5.8, 1.95, TINT);
  s.addText([
    { text: "数据安全边界", options: { fontSize: 15, bold: true, color: TEXT, breakLine: true } },
    { text: "服务仅绑定 127.0.0.1;唯一可选外呼是 LLM API(偏好慢通道/仲裁/问答),不配置 Key 即完全离线;遗忘与审计操作全部留痕可查。", options: { fontSize: 12.5, color: MUTED } },
  ], { x: 7.25, y: 4.45, w: 5.3, h: 1.6, fontFace: F, paraSpaceAfter: 8, margin: 0 });
}

// ================= S10 创新点1 经验复用层 =================
{
  const s = p.addSlide();
  pageChrome(s, 10, "创新点 ①");
  slideTitle(s, "创新点 ① 借鉴 WikiSkill", "经验复用层:让 Agent 把踩过的坑变成能力");
  const steps = [["轨迹", "带成败标注的工具调用轨迹(真实执行,harness 金标核验)"], ["编译", "LLM 提炼「做法 / 避坑」经验卡(规则统计兜底)"], ["注入", "检索提权 + Agent 中枢分发置顶(已验证优先)"], ["门控", "复用 ≥2 次且零失败 → 自动转「已验证」;有败 → 回草稿"], ["演化", "经验卡写入即走冲突管道。环境变了经验自己更新,不会误删"]];
  steps.forEach((st, i) => {
    const y = 1.5 + i * 0.98;
    s.addShape(p.shapes.ROUNDED_RECTANGLE, { x: M, y: y + 0.04, w: 1.15, h: 0.6, fill: { color: i === 4 ? ACCENT : PRIMARY }, rectRadius: 0.08 });
    s.addText(st[0], { x: M, y: y + 0.04, w: 1.15, h: 0.6, fontSize: 14, fontFace: F, color: "FFFFFF", bold: true, align: "center", valign: "middle", margin: 0 });
    s.addText(st[1], { x: M + 1.35, y, w: 5.15, h: 0.9, fontSize: 12.5, fontFace: F, color: TEXT, valign: "middle", margin: 0 });
  });
  pic(s, A + "/know.png", 7.25, 1.45, 3.55, "「知识与冲突」页:经验卡(做法/避坑)+ 冲突台账");
  s.addShape(p.shapes.RECTANGLE, { x: 7.2, y: 5.55, w: 5.6, h: 0.95, fill: { color: TINT } });
  s.addText([
    { text: "对照实验:Agent EM 90.0% → 96.7%(+6.7pts,零回退)", options: { bold: true, color: ACCENT, fontSize: 14, breakLine: true } },
    { text: "而未编译的原始记忆检索只有 86.7%(-3.3pts)。先编译、再注入,记忆才能变成能力", options: { color: TEXT, fontSize: 11.5 } },
  ], { x: 7.4, y: 5.62, w: 5.2, h: 0.85, fontFace: F, margin: 0 });
  srcNote(s, "来源:WikiSkill(arXiv 2608.27454)经验编译思想的三层落地;三臂对照 DSH headless + MiniMax-M3,n=30,详见 AGENT_EVAL.md。");
}

// ================= S11 创新点2 检索 =================
{
  const s = p.addSlide();
  pageChrome(s, 11, "创新点 ②");
  slideTitle(s, "创新点 ② 关联检索", "RRF 三路融合 + 通道自适应");
  const chans = [["向量 ANN", "sqlite-vec,768 维麒麟嵌入", "语义/中文强"], ["BM25 全文", "tantivy,词干化+停用词", "词面/英文对话强"], ["图扩展", "实体共现边,一跳关联", "多跳关联(门控)"]];
  chans.forEach((c, i) => {
    const x = M + i * 2.3;
    card(s, x, 1.55, 2.1, 1.9);
    s.addText([
      { text: c[0], options: { fontSize: 14.5, bold: true, color: TEXT, breakLine: true } },
      { text: c[1], options: { fontSize: 11, color: MUTED, breakLine: true } },
      { text: c[2], options: { fontSize: 11, color: ACCENT, bold: true } },
    ], { x: x + 0.15, y: 1.7, w: 1.8, h: 1.6, fontFace: F, paraSpaceAfter: 6, margin: 0 });
  });
  s.addText("→", { x: 7.05, y: 2.2, w: 0.5, h: 0.5, fontSize: 24, fontFace: F, color: MUTED, align: "center", margin: 0 });
  card(s, 7.6, 1.55, 5.2, 1.9, TINT);
  s.addText([
    { text: "RRF 融合(k=60)→ 偏好重排", options: { fontSize: 15, bold: true, color: ACCENT, breakLine: true } },
    { text: "名次级融合免调权;top_k 自适应扩窗;检索延迟 p50 16.5ms", options: { fontSize: 12, color: TEXT } },
  ], { x: 7.85, y: 1.8, w: 4.7, h: 1.5, fontFace: F, paraSpaceAfter: 6, margin: 0 });
  card(s, M, 3.85, 6.05, 2.3);
  s.addText([
    { text: "通道贡献随语料自适应(消融发现)", options: { fontSize: 14.5, bold: true, color: TEXT, breakLine: true } },
    { text: "英文对话:BM25 强;中文/语义查询:向量强;图通道在对话语料为负贡献 → 可门控", options: { fontSize: 12.5, color: MUTED, breakLine: true } },
    { text: "检索优化链:47.8% → 52.5%(词干化)→ 59.0%(邻近轮次窗口索引)", options: { fontSize: 12.5, color: TEXT } },
  ], { x: M + 0.25, y: 4.05, w: 5.55, h: 1.95, fontFace: F, paraSpaceAfter: 8, margin: 0 });
  card(s, 6.85, 3.85, 5.95, 2.3);
  s.addText([
    { text: "两个口径,如实报告", options: { fontSize: 14.5, bold: true, color: TEXT, breakLine: true } },
    { text: "自建书山中文知识库(120 回,严格 test@5):96.1% —— 达成 ≥85% 指标", options: { fontSize: 12.5, color: OK, bold: true, breakLine: true } },
    { text: "公开 LoCoMo 英文超长对话(免 LLM 严格协议,1,986 问全量):59.0% —— 公开可比上限,同步如实报告并给出优化方向", options: { fontSize: 12.5, color: MUTED } },
  ], { x: 7.1, y: 4.05, w: 5.45, h: 1.95, fontFace: F, paraSpaceAfter: 8, margin: 0 });
  srcNote(s, "来源:REPORT.md §一/§〇一;hit@5 全量逐题明细与消融矩阵(results/locomo-*.json、matrix/)均可复核。");
}

// ================= S12 量化评测体系 =================
{
  const s = p.addSlide();
  pageChrome(s, 12, "量化评测(条款 7)");
  slideTitle(s, "条款 7 · 量化评测机制", "五项硬指标全部达成,逐题可复核");
  const rows = [
    ["偏好提取准确率", "≥85%", "98.0%(PrefEval 存储协议)+ 会话专项 100%", true],
    ["知识检索召回率", "≥85%", "96.1%(自建书山中文集,严格 test@5)", true],
    ["知识检索响应时间", "≤500ms", "p50 16.5ms / p95 29.4ms(442 查询 0 超标)", true],
    ["知识冲突处理正确率", "≥88%", "93.3%(30 组受控用例 28/30)", true],
    ["记忆能力(MemBench)", "≥85%", "94.0%(r8 对齐协议全量 100 题)", true],
  ];
  const header = ["指标", "要求", "实测", "达成"].map(t => ({ text: t, options: { fill: { color: PRIMARY }, color: "FFFFFF", bold: true, fontFace: F, fontSize: 13.5 } }));
  const body = rows.map(r => [
    { text: r[0], options: { fontFace: F, fontSize: 13, color: TEXT } },
    { text: r[1], options: { fontFace: F, fontSize: 13, color: MUTED, align: "center" } },
    { text: r[2], options: { fontFace: F, fontSize: 13, color: TEXT } },
    { text: "✓ 达成", options: { fontFace: F, fontSize: 13, color: OK, bold: true, align: "center" } },
  ]);
  s.addTable([header, ...body], { x: M, y: 1.55, w: 12.33, colW: [2.6, 1.3, 6.93, 1.5], rowH: 0.62, border: { pt: 0.5, color: LINE }, valign: "middle", margin: 0.08 });
  s.addText([
    { text: "评测方法学:", options: { bold: true, color: TEXT } },
    { text: "七站公开/自建基准(LoCoMo、MemBench、PrefEval、MAB-conflict、书山·三国、MS MARCO、TrecQA…)+ 与 mem0/iwe/memmy 的同题四系统矩阵 + 灌入校验门(基线静默失败检测)+ 原始逐题落盘,全部可复现。", options: { color: MUTED } },
  ], { x: M, y: 5.75, w: 12.33, h: 0.85, fontSize: 12.5, fontFace: F, margin: 0 });
  srcNote(s, "来源:评测总报告 v4(REPORT.md)§五硬指标表;对比口径与历史轮次保留于同文件供复核。");
}

// ================= S13 Agent 三臂对照 =================
{
  const s = p.addSlide();
  pageChrome(s, 13, "对 Agent 的提升");
  slideTitle(s, "对照实验", "经验复用层让真实 Agent 更强:+6.7pts 零回退");
  s.addChart(p.charts.BAR, [{ name: "EM 全对率(%)", labels: ["裸 Agent(base)", "+原始记忆检索(mem)", "+经验复用层(exp)"], values: [90.0, 86.7, 96.7] }], {
    x: M, y: 1.6, w: 6.6, h: 4.4, barDir: "col", varyColors: true,
    chartColors: ["9AB2D6", "C9D4E6", ACCENT],
    catAxisLabelColor: MUTED, valAxisLabelColor: MUTED, catAxisLabelFontFace: F, valAxisLabelFontFace: F, dataLabelFontFace: F,
    valAxisMaxVal: 100, valGridLine: { color: "E2E8F0", size: 0.5 }, catGridLine: { style: "none" },
    showValue: true, dataLabelPosition: "outEnd", dataLabelColor: TEXT, showLegend: false,
    showTitle: true, title: "工具选择任务 EM(DSH Agent + MiniMax-M3,n=30)", titleFontFace: F, titleFontSize: 13, titleColor: TEXT,
  });
  const pts = [
    ["避坑卡纠正系统性漏选", "test-08:经验卡明确\"该任务需同时调用 list 与 detail\",修复漏选"],
    ["经验卡给出正确 API 形态", "test-25:裸 Agent 把 {sport} 模板参数臆造成假 API 名"],
    ["原始记忆检索反而 -3.3pts", "未蒸馏的召回噪声+工具开销;\"先编译再注入\"是关键路径"],
    ["强模型饱和效应", "gpt-5.6-sol 裸 Agent 已 96.7%——经验价值在模型能力不足处最大化"],
  ];
  pts.forEach((pt, i) => {
    const y = 1.6 + i * 1.12;
    s.addText([
      { text: pt[0], options: { fontSize: 14, bold: true, color: i === 2 ? AMBER : TEXT, breakLine: true } },
      { text: pt[1], options: { fontSize: 11.5, color: MUTED } },
    ], { x: 7.4, y, w: 5.4, h: 1.05, fontFace: F, margin: 0 });
  });
  srcNote(s, "局限如实报告:n=30 单种子、工具池 26 张卡;接入成本 p50 +3.2s(一次工具往返)。来源:AGENT_EVAL.md,原始输出逐题落盘。");
}

// ================= S14 性能优化 A/B =================
{
  const s = p.addSlide();
  pageChrome(s, 14, "性能优化");
  slideTitle(s, "端侧轻量化论证", "热路径微优化:p50 -28%,逐题召回零翻转");
  s.addChart(p.charts.BAR, [
    { name: "优化前", labels: ["p50(ms)", "p95(ms)"], values: [22.8, 40.2] },
    { name: "优化后", labels: ["p50(ms)", "p95(ms)"], values: [16.5, 29.4] },
  ], {
    x: M, y: 1.6, w: 6.6, h: 4.3, barDir: "col",
    chartColors: ["9AB2D6", ACCENT],
    catAxisLabelColor: MUTED, valAxisLabelColor: MUTED, catAxisLabelFontFace: F, valAxisLabelFontFace: F, dataLabelFontFace: F,
    valGridLine: { color: "E2E8F0", size: 0.5 }, catGridLine: { style: "none" },
    showValue: true, dataLabelPosition: "outEnd", dataLabelColor: TEXT,
    showLegend: true, legendPos: "b", legendFontFace: F, legendColor: MUTED,
    showTitle: true, title: "检索延迟分位(442 查询·暖态)", titleFontFace: F, titleFontSize: 13, titleColor: TEXT,
  });
  const items = [
    ["三批次改动", "spawn_blocking+LLM 探测后台化 → 查询向量 LRU → 候选批量拉取/RRF HashMap 化/批量事务+PRAGMA"],
    ["严格 A/B", "新旧二进制全暖态盲测,排除嵌入服务缓存混杂;优化前二进制留档对照"],
    ["零回归", "442 题逐题 hit@5 与优化前完全一致(0 翻转);core 单测 30/30"],
    ["启动优化", "LLM 探测后台化:端口 255ms 即就绪,不再被探测阻塞最长 60s"],
  ];
  items.forEach((it, i) => {
    const y = 1.6 + i * 1.12;
    s.addText([
      { text: it[0], options: { fontSize: 14, bold: true, color: TEXT, breakLine: true } },
      { text: it[1], options: { fontSize: 11.5, color: MUTED } },
    ], { x: 7.4, y, w: 5.4, h: 1.05, fontFace: F, margin: 0 });
  });
  srcNote(s, "来源:PERF.md(AlloyStack 思路移植:启动优化+热路径去冗余);原始逐题数据 perf_*.json 落盘可复核。");
}

// ================= S15 界面演示 =================
{
  const s = p.addSlide();
  pageChrome(s, 15, "软件实现");
  slideTitle(s, "软件实现", "中文管理台:七页覆盖全部能力,开箱即见");
  pic(s, A + "/dash.png", M, 1.5, 3.7, "总览:一键灌入演示数据(隔离场景,可一键清除)");
  pic(s, A + "/arena.png", 6.82, 1.5, 3.7, "评测对比:内置数据集一键出分 + 指标达成");
  s.addText([
    { text: "七页动线:总览 → 记忆库 → 偏好中心 → 知识与冲突 → 遗忘与安全 → 评测对比 → Agent 中枢", options: { fontSize: 12.5, color: MUTED, bullet: bu(), breakLine: true } },
    { text: "全新安装首启为空库 → 「灌入演示数据」按钮一键生成四源样例(含必现冲突与经验卡)", options: { fontSize: 12.5, color: MUTED, bullet: bu(), breakLine: true } },
    { text: "零竞赛字样,面向普世用户;浏览器直达,无需安装客户端", options: { fontSize: 12.5, color: MUTED, bullet: bu() } },
  ], { x: M, y: 5.72, w: 12.33, h: 0.95, fontFace: F, paraSpaceAfter: 5, margin: 0 });
  srcNote(s, "截图为 0.1.0 真机运行实拍(银河麒麟桌面 V11);更多页面见演示视频与用户手册。", 6.78);
}

// ================= S16 应用案例 =================
{
  const s = p.addSlide();
  pageChrome(s, 16, "应用案例");
  slideTitle(s, "真实场景应用", "从工具轨迹到知识库:三个落地案例");
  const cases = [
    ["案例一 · Agent 记忆插件(真实集成)", "DeepSeek Harness(DSH)真机接入 lymem MCP:Agent 获得记忆检索/写入/遗忘/经验卡 8 个工具;任务轨迹自动回写、经验卡编译复用 —— 三臂对照实测 EM 90.0%→96.7%。"],
    ["案例二 · 整本书知识库(长文档沉淀)", "《三国演义》57.8 万字 / 108 回灌入(532 秒,端侧嵌入),历史事件问答准确并引用回目;支撑自建书山中文检索集(96.1%)与整本书问答评测。"],
    ["案例三 · 办公记忆(开箱演示)", "会议纪要转 PDF 归档的成功轨迹与批量压缩截图的失败轨迹,一键灌入后编译出「做法/避坑」经验卡;演示冲突(打印服务器改址)完整走完检测→仲裁→版本化。"],
  ];
  cases.forEach((c, i) => {
    const y = 1.55 + i * 1.62;
    card(s, M, y, 12.33, 1.42, i === 0 ? TINT : "FFFFFF");
    s.addText(c[0], { x: M + 0.3, y: y + 0.12, w: 11.7, h: 0.42, fontSize: 15.5, fontFace: F, color: ACCENT, bold: true, margin: 0 });
    s.addText(c[1], { x: M + 0.3, y: y + 0.56, w: 11.7, h: 0.8, fontSize: 12.5, fontFace: F, color: TEXT, margin: 0 });
  });
  srcNote(s, "案例一为真实第三方 Agent 框架集成(非模拟);原始会话与评测输出落盘 results/agent_ab/。");
}

// ================= S17 总结 =================
{
  const s = p.addSlide();
  s.background = { color: DARK };
  s.addText("总结", { x: M, y: 0.55, w: 4, h: 0.5, fontSize: 16, fontFace: F, color: LIGHT, margin: 0 });
  s.addText("记忆存在本机,离线也能用,每个指标都可复核", { x: M, y: 1.0, w: W - 2 * M, h: 0.75, fontSize: 34, fontFace: F, color: "FFFFFF", bold: true, margin: 0 });
  const left = [
    ["全需求覆盖", "七项条款逐条实现,五项硬指标全部达成"],
    ["两大创新", "经验复用层(Agent +6.7pts)· RRF 三路自适应检索"],
    ["端侧轻量", "p50 16.5ms / 二进制 19.7MB / 单文件存储 / 完全离线可运行"],
    ["真机验证", "银河麒麟桌面 V11 适配测试通过,deb+便携包双形态"],
  ];
  left.forEach((it, i) => {
    const y = 2.15 + i * 1.05;
    s.addText([
      { text: it[0] + "   ", options: { fontSize: 17, bold: true, color: "FFD479" } },
      { text: it[1], options: { fontSize: 13.5, color: LIGHT } },
    ], { x: M, y, w: 7.3, h: 0.95, fontFace: F, valign: "middle", margin: 0 });
  });
  card(s, 8.3, 2.15, 4.5, 4.0, "1D2E4C");
  s.addText("交付物清单", { x: 8.6, y: 2.35, w: 3.9, h: 0.4, fontSize: 15, fontFace: F, color: "FFFFFF", bold: true, margin: 0 });
  s.addText([
    { text: "源代码(完整注释,MIT)", options: { fontSize: 12.5, color: LIGHT, bullet: bu(), breakLine: true } },
    { text: "技术方案 + 效果验证报告(word/pdf)", options: { fontSize: 12.5, color: LIGHT, bullet: bu(), breakLine: true } },
    { text: "用户手册 + 麒麟适配测试报告", options: { fontSize: 12.5, color: LIGHT, bullet: bu(), breakLine: true } },
    { text: "评测总报告 v4(逐题数据可复核)", options: { fontSize: 12.5, color: LIGHT, bullet: bu(), breakLine: true } },
    { text: "效果演示视频 + 本报告(PPT)", options: { fontSize: 12.5, color: LIGHT, bullet: bu() } },
  ], { x: 8.6, y: 2.85, w: 3.9, h: 3.1, fontFace: F, paraSpaceAfter: 10, margin: 0 });
  s.addText("麟忆尽智 · 谢谢观看", { x: M, y: 6.6, w: 6, h: 0.5, fontSize: 18, fontFace: F, color: "FFFFFF", bold: true, margin: 0 });
  s.addText("提交邮箱:wangyu1@kylinos.cn", { x: 8.3, y: 6.65, w: 4.5, h: 0.4, fontSize: 11.5, fontFace: F, color: "7E96BC", align: "right", margin: 0 });
}

p.writeFile({ fileName: "项目报告-麟忆尽智lymem.pptx" }).then(() => console.log("PPT done"));
