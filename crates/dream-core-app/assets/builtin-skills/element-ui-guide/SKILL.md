---
name: element-ui-guide
display_name: "Element UI 组件速查与代码生成"
description: Element UI (Vue 2) 全量组件速查与代码生成技能。当用户需要查询 Element UI 某个组件的用法、属性、事件、方法、插槽时触发；当用户需要生成基于 Eleme。触发：用户提出「Element UI 组件速查与代码生成」相关需求时使用（常见说法：Element UI 组件速查与代码生成、Element UI 组件速查与代码生成；英文：element/ui/guide）；不要用于“生成或编辑 Word/Excel/PPT 文档文件”（改用 officecli-docx / officecli-xlsx / officecli-pptx），也不要用于与本技能无关的其他任务。需要用户提供：明确的任务描述，以及必要的输入文件或数据。
---


# Element UI Guide

## 概述

本技能提供 Element UI（Vue 2 版，https://element.eleme.cn）全量组件的速查手册与代码生成能力。
支持两种使用模式：组件速查（查询单个组件用法）和代码生成（根据需求生成完整 .vue 单文件组件）。
布局借鉴 Avue 风格（搜索栏 + 操作按钮 + 表格 + 分页），但全部使用原生 Element 组件实现。

## 触发场景

1. 用户询问某个 Element UI 组件怎么用（如"el-table 怎么自定义列"、"el-cascader 怎么动态加载"）
2. 用户要求生成基于 Element UI 的页面（如"生成一个用户管理页面"、"做一个商品列表"）
3. 用户提到 el- 前缀组件名
4. 用户要求 CRUD 列表页、表单页、详情页等管理后台页面

## 代码规范

所有生成的代码必须遵循以下规范：

- **框架**：Vue 2 + Element UI，Options API（data / methods / computed / watch）
- **语言**：JavaScript，不使用 TypeScript
- **样式**：使用 Element UI 默认样式，不自定义主题，`<style>` 标签不加 `scoped` 除非用户要求
- **组件属性**：使用 Element UI 全量属性，不省略常用属性
- **API 请求**：使用 axios 封装，通过 `this.$api` 或 `import` 方式调用
- **分页数据结构**：后端返回 `{ records, total, current, size }` 格式（MyBatis-Plus Page 对象）
- **加载状态**：表格使用 `v-loading` 指令
- **权限指令**：不添加任何权限指令
- **代码注释**：关键逻辑处添加中文注释

## 工作流

### 模式一：组件速查

当用户查询某个组件的用法时：

1. 读取 `references/component-reference.md`，找到对应组件的精简手册
2. 返回组件的核心属性（Attributes）、事件（Events）、方法（Methods）、插槽（Slots）
3. 附带一个最小可运行的示例代码片段
4. 对于复杂组件，附加官网链接供深入查阅

### 模式二：代码生成

当用户要求生成页面或组件时：

1. 判断生成类型：
   - **CRUD 列表页**：参考 `assets/crud-page.vue` 模板，遵循 Avue 布局格式
   - **表单页**：参考 `assets/form-page.vue` 模板
   - **详情页**：参考 `assets/detail-page.vue` 模板
   - **单组件示例**：从 `references/component-reference.md` 查询组件用法，生成示例
2. 根据用户需求定制：字段名、搜索条件、表格列、操作按钮、表单字段等
3. 生成完整 .vue 单文件组件（`<template>` + `<script>` + `<style>`）
4. 如果用户明确说"只生成 template"或"只要片段"，则按需缩减

### Avue 布局格式（CRUD 页面）

CRUD 列表页采用以下布局结构（借鉴 Avue，原生 Element 实现）：

```
搜索栏（el-form inline，可折叠）
  └─ 输入框 / 下拉 / 日期等搜索条件 + [搜索] [重置] 按钮
操作按钮栏
  └─ [新增] [批量删除] [导出] [刷新] 等按钮
数据表格（el-table，带 v-loading）
  └─ 多选列 / 序号列 / 数据列 / 操作列（编辑/删除）
分页（el-pagination）
  └─ total / sizes / jumper / prev / next
弹窗（el-dialog）
  └─ 新增/编辑表单（el-form）
```

## 组件速查手册

完整的组件速查手册位于 `references/component-reference.md`，覆盖 Element UI 全量组件，按以下分类组织：

- **Basic**：Layout / Container / Button / Link / Icon
- **Form**：Radio / Checkbox / Input / Number / Select / Cascader / Switch / Slider / DatePicker / DateTimePicker / Upload / Rate / ColorPicker / Transfer / Form / TimePicker / Tag / InputNumber
- **Data**：Table / Tag / Progress / Tree / Pagination / Badge / Avatar / Skeleton / Empty / Image / Collapse
- **Notice**：Alert / Loading / Message / MessageBox / Notification
- **Navigation**：Menu / Tabs / Breadcrumb / PageHeader / Dropdown / Steps
- **Others**：Dialog / Tooltip / Popover / Popconfirm / Card / Carousel / Collapse / Timeline / Divider / Backtop / Drawer / Calendar

每个组件包含：核心属性表、事件表、方法表、插槽表、最小示例代码、官网链接。

## 资源文件

### references/
- `component-reference.md`：Element UI 全量组件速查手册（核心属性 + 事件 + 方法 + 插槽 + 示例）

### assets/
- `crud-page.vue`：CRUD 列表页模板（Avue 布局 + 原生 Element 实现）
- `form-page.vue`：表单页模板（el-form 全量属性）
- `detail-page.vue`：详情页模板（描述列表布局）

## 注意事项

- 本技能仅适用于 Element UI（Vue 2），不适用于 Element Plus（Vue 3）
- Element UI 版本以 2.15.x 为基准（最后一个稳定版）
- 生成代码时如遇到业务字段不明确，用合理的占位符并注释说明
- 表格操作列默认用文字按钮（编辑/删除），不用图标，除非用户要求
- 搜索栏默认可折叠，通过 `showSearch` 控制显隐
- 分页默认 `layout="total, sizes, prev, pager, next, jumper"`
