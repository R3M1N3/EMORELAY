//! EMORELAY 共享类型、错误、协议常量。
//!
//! `control::v1` 模块由 tonic-build 在编译期生成，包含 protobuf 消息与 gRPC 客户端 /
//! 服务端 trait。两端（panel-server / node-agent）共用，避免协议漂移。

pub mod control {
    pub mod v1 {
        tonic::include_proto!("emorelay.control.v1");
    }
}
