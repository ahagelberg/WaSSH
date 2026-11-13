fn main() {
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../../res/WaSSH.ico")
        .compile()
        .expect("the program icon is compiled into the executable");
}
