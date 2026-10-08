// 发布构建下不弹控制台窗口。
//
// 注意这行只在 release 生效：debug 构建保留控制台，因为首次跑起来时最常需要的
// 就是看一眼模型加载到哪一步、麦克风有没有被系统拒绝。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    prompter_lib::run()
}
