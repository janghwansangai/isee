// 윈도우 배포판에서 콘솔 창이 함께 뜨지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    isee_app_lib::run()
}
