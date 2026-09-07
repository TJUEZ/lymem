//! 演示数据一键灌入/清除：让全新安装"开箱即见"。
//!
//! 全部数据隔离在「演示」场景（scene="演示"）：
//! - 灌入 = 多源事件（工具轨迹/会话/手动配置/行为）+ 知识条目（含一对必现冲突）；
//! - 清除 = 该场景记忆硬删除 + 演示偏好版本回收（审计留痕不删）。
//! 轨迹带任务标签，可直接在「知识与冲突」页编译出经验卡（做法/避坑各一）。

use lymem_core::forget::{ForgetMode, ForgetScope};
use lymem_core::ingest::IngestEvent;
use lymem_core::llm_hook::LlmJudge;
use lymem_core::model::{MemoryKind, MemoryRecord, Tier};
use lymem_core::{MemoryStore, CoreError};
use serde_json::{json, Value};

pub const DEMO_SCENE: &str = "演示";

/// 演示事件：覆盖四类数据源。两条任务的工具轨迹供经验卡编译
/// （"会议纪要转PDF归档"全成功 → 做法；"批量压缩截图"先败后成 → 避坑）。
fn demo_events() -> Vec<IngestEvent> {
    let tr = |tool: &str, task: &str, ok: bool, output: &str| IngestEvent::ToolResult {
        tool: tool.into(),
        task: task.into(),
        args: Value::Null,
        ok,
        output: output.into(),
        scene: DEMO_SCENE.into(),
        source: "demo".into(),
    };
    vec![
        tr("libreoffice", "会议纪要转PDF归档", true, "soffice --headless --convert-to pdf 会议纪要-09月.docx：导出成功，生成 会议纪要-09月.pdf"),
        tr("libreoffice", "会议纪要转PDF归档", true, "soffice --headless --convert-to pdf 会议纪要-10月.docx：导出成功"),
        tr("file-roller", "会议纪要转PDF归档", true, "将 PDF 与源 docx 打包归档到 ~/文档/纪要归档/2026-10/ 成功"),
        tr("imagemagick", "批量压缩截图", false, "magick 截图01.png -quality 85 截图01.jpg：报错 no decode delegate（jpg 委托缺失且压缩率不足）"),
        tr("imagemagick", "批量压缩截图", true, "改用 webp 输出：magick 截图01.png -quality 85 截图01.webp 成功，体积降 76%"),
        tr("imagemagick", "批量压缩截图", true, "批量 12 张截图全部转 webp 成功，总耗时 3.2s"),
        IngestEvent::Conversation {
            role: "user".into(),
            text: "以后导出文件都用 PDF 格式，不要再用 docx 了。".into(),
            scene: DEMO_SCENE.into(),
            source: "demo".into(),
            meta: None,
            window_context: None,
        },
        IngestEvent::Conversation {
            role: "user".into(),
            text: "我习惯把系统调成深色主题，晚上看屏幕舒服。".into(),
            scene: DEMO_SCENE.into(),
            source: "demo".into(),
            meta: None,
            window_context: None,
        },
        IngestEvent::ManualConfig { key: "导出格式".into(), value: json!("PDF"), scene: DEMO_SCENE.into(), source: "demo".into() },
        IngestEvent::ManualConfig { key: "界面主题".into(), value: json!("深色"), scene: DEMO_SCENE.into(), source: "demo".into() },
        IngestEvent::ManualConfig { key: "文件排序".into(), value: json!("按修改时间"), scene: DEMO_SCENE.into(), source: "demo".into() },
        IngestEvent::UserBehavior {
            event: "夜间办公高峰".into(),
            detail: "22:00-24:00 文档编辑占比 70%，偏好安静的通知方式".into(),
            scene: DEMO_SCENE.into(),
            source: "demo".into(),
        },
        IngestEvent::UserBehavior {
            event: "常用应用".into(),
            detail: "WPS 办公、火狐浏览器、文件管理器、截图工具".into(),
            scene: DEMO_SCENE.into(),
            source: "demo".into(),
        },
    ]
}

/// 演示知识：工作流 / 模板 / 案例 + 一对必现冲突的新旧地址（同题改值，
/// 实体交集保证命中冲突检测的实体门，末条入库时触发仲裁并版本化）。
fn demo_knowledge() -> Vec<(&'static str, MemoryKind, &'static str, Vec<&'static str>)> {
    vec![
        ("月度报销流程", MemoryKind::Workflow, "工作流：1) 扫描发票并按「日期-事项」重命名；2) 登录 OA 填写报销单；3) 上传发票影像；4) 提交部门主管审批；5) 抄送财务归档。", vec!["报销", "OA"]),
        ("会议纪要模板", MemoryKind::Template, "模板：会议主题 / 时间地点 / 参会人 / 议题与结论 / 待办事项（负责人+截止日）/ 下次会议安排。", vec!["会议纪要"]),
        ("打印机卡纸处理案例", MemoryKind::Case, "案例：先开前盖取出卡纸并检查进纸轮；连续卡纸时清理搓纸轮，仍无效则更换搓纸轮组件。", vec!["打印机"]),
        ("办公区打印服务器地址", MemoryKind::Fact, "办公区打印服务器地址为 192.168.1.100，端口 631，协议 IPP。", vec!["打印服务器"]),
        ("办公区打印服务器地址", MemoryKind::Fact, "办公区打印服务器地址为 192.168.1.102，端口 631，协议 IPP。", vec!["打印服务器"]),
    ]
}

/// 灌入演示数据（幂等：场景内已有记忆则跳过）。
pub fn seed_demo(store: &MemoryStore, judge: &dyn LlmJudge) -> Result<(usize, usize, usize, bool), CoreError> {
    if !store.list(None, Some(DEMO_SCENE), false, 1)?.is_empty() {
        let n = store.list(None, Some(DEMO_SCENE), false, 10_000)?.len();
        return Ok((n, 0, 0, true));
    }
    let ids = lymem_core::ingest::ingest_events(store, &demo_events())?;
    let mut knowledge = 0usize;
    let mut conflicts = 0usize;
    for (title, kind, content, entities) in demo_knowledge() {
        let mut rec = MemoryRecord::new(Tier::Knowledge, kind, title.to_string(), content.to_string());
        rec.scene = DEMO_SCENE.into();
        rec.entities = entities.into_iter().map(String::from).collect();
        let (_, outcomes) = store.put_knowledge_with_conflicts(rec, Some(judge), 0.8)?;
        knowledge += 1;
        conflicts += outcomes.len();
    }
    store.audit("demo_seed", &json!({"memories": ids.len(), "knowledge": knowledge, "conflicts": conflicts}))?;
    // 给时间机器留一个变化点：若「导出格式」只有初版，追加 v2（此后用户可拖动滑块看到差异）
    if let Ok(h) = store.preference_history("导出格式") {
        if h.len() == 1 {
            let _ = store.set_preference(&lymem_core::preference::PreferenceSet {
                key: "导出格式".into(),
                value: json!("DOCX 初稿，终稿转 PDF"),
                evidence: vec![json!({"source": "demo", "note": "对外提交文件改用 PDF 终稿"})],
                source: "demo".into(),
                confidence: 0.9,
                scenes: vec![DEMO_SCENE.to_string()],
                force: true,
            });
        }
    }
    Ok((ids.len(), knowledge, conflicts, false))
}

/// 清除演示数据：场景记忆硬删除 + 演示偏好版本回收 + 孤儿偏好键清理。
pub fn clear_demo(store: &MemoryStore) -> Result<usize, CoreError> {
    let scope = ForgetScope { scenes: vec![DEMO_SCENE.into()], ..Default::default() };
    let removed = store.forget_execute(&scope, ForgetMode::Purge)?;
    {
        let conn = store.conn.lock().unwrap();
        // 删除场景标记为「演示」的偏好版本，回收孤儿键，并把失去当前版本的键指回剩余最新版本
        conn.execute("DELETE FROM pref_versions WHERE scenes LIKE ?1", [format!("%{DEMO_SCENE}%")])?;
        conn.execute(
            "UPDATE preferences SET current_version = COALESCE(\
             (SELECT MAX(version) FROM pref_versions v WHERE v.pref_id = preferences.id), 0) \
             WHERE current_version NOT IN (SELECT version FROM pref_versions WHERE pref_id = preferences.id)",
            [],
        )?;
        conn.execute("DELETE FROM preferences WHERE id NOT IN (SELECT DISTINCT pref_id FROM pref_versions)", [])?;
    }
    store.audit("demo_clear", &json!({"removed": removed}))?;
    Ok(removed)
}
