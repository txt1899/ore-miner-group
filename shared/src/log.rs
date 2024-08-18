use tracing_subscriber::{
    fmt::{format, time::ChronoLocal},
    EnvFilter,
};

pub fn init_log() {
    let format = format::format()
		.with_level(true) // 显示日志级别
		.with_target(false) // 隐藏日志目标
		//.with_thread_ids(true) // 显示线程 ID
		.with_timer(ChronoLocal::new("[%m-%d %H:%M:%S%.3f]".to_string())) // 使用本地时间格式化时间戳
		//.with_thread_names(true) // 显示线程名称
		.compact(); // 使用紧凑的格式

    let env_filter = EnvFilter::from_default_env()
        //.add_directive("mine_server=debug".parse().unwrap())
        //.add_directive("mine_client=debug".parse().unwrap())
        .add_directive("info".parse().unwrap()); // 默认其他依赖只输出 warn 级别以上的日志
    tracing_subscriber::fmt().with_env_filter(env_filter).event_format(format).init();
}
