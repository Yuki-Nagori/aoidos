# 角色身份与权限

角色模型把角色身份和真值、角色认知、角色对外呈现、开场职责分开管理。每个参与剧情的角色（包括玩家操控的主角）都必须在当前 run 有一个冻结的个人档案；这不代表玩家可以查看它，也不代表模型的每个请求都能读取它。

| 主题                 | 文档                                        | 定义                                         |
| -------------------- | ------------------------------------------- | -------------------------------------------- |
| 身份与个人真值       | [档案](profiles.md)                         | 稳定 ID、主角绑定、真值来源、字段和 run 冻结 |
| 认知、公开形象与揭露 | [认知与呈现](knowledge-and-presentation.md) | 知道什么、对谁说什么、何时可以揭露           |
| 首个引导 NPC         | [引导职责](guide-npc.md)                    | 固定 / 生成选择、职责提示和关系边界          |

```text
canonical identity ≠ private truth ≠ character knowledge ≠ public claim
presence ≠ knowledge; dialogue ≠ evidence; a profile ≠ permission
```

玩家操控权由 run 的 protagonist binding 授权；它与 actor 的故事身份分开存储。主角可以不清楚自身真相，玩家也不因操控主角自动全知。角色只有在可信场景策略授权后才算在场；在场本身也不授予无关秘密的获知权。
