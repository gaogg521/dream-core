# Element UI 全量组件速查手册

> 基于 Element UI 2.15.x（Vue 2）
> 官网：https://element.eleme.cn

---

## 目录

- [Basic 基础组件](#basic-基础组件)
  - [Layout 布局](#layout-布局)
  - [Container 布局容器](#container-布局容器)
  - [Button 按钮](#button-按钮)
  - [Link 文字链接](#link-文字链接)
  - [Icon 图标](#icon-图标)
- [Form 表单组件](#form-表单组件)
  - [Radio 单选框](#radio-单选框)
  - [Checkbox 多选框](#checkbox-多选框)
  - [Input 输入框](#input-输入框)
  - [InputNumber 计数器](#inputnumber-计数器)
  - [Select 选择器](#select-选择器)
  - [Cascader 级联选择器](#cascader-级联选择器)
  - [Switch 开关](#switch-开关)
  - [Slider 滑块](#slider-滑块)
  - [TimePicker 时间选择器](#timepicker-时间选择器)
  - [DatePicker 日期选择器](#datepicker-日期选择器)
  - [DateTimePicker 日期时间选择器](#datetimepicker-日期时间选择器)
  - [Upload 上传](#upload-上传)
  - [Rate 评分](#rate-评分)
  - [ColorPicker 颜色选择器](#colorpicker-颜色选择器)
  - [Transfer 穿梭框](#transfer-穿梭框)
  - [TimePicker 时间选择器](#timepicker-时间选择器-1)
  - [Tag 标签](#tag-标签)
  - [Form 表单](#form-表单)
- [Data 数据展示](#data-数据展示)
  - [Table 表格](#table-表格)
  - [Progress 进度条](#progress-进度条)
  - [Tree 树形控件](#tree-树形控件)
  - [Pagination 分页](#pagination-分页)
  - [Badge 标记](#badge-标记)
  - [Avatar 头像](#avatar-头像)
  - [Skeleton 骨架屏](#skeleton-骨架屏)
  - [Empty 空状态](#empty-空状态)
  - [Image 图片](#image-图片)
  - [Collapse 折叠面板](#collapse-折叠面板)
- [Notice 提示](#notice-提示)
  - [Alert 警告](#alert-警告)
  - [Loading 加载](#loading-加载)
  - [Message 消息提示](#message-消息提示)
  - [MessageBox 消息弹框](#messagebox-消息弹框)
  - [Notification 通知](#notification-通知)
- [Navigation 导航](#navigation-导航)
  - [Menu 导航菜单](#menu-导航菜单)
  - [Tabs 标签页](#tabs-标签页)
  - [Breadcrumb 面包屑](#breadcrumb-面包屑)
  - [PageHeader 页头](#pageheader-页头)
  - [Dropdown 下拉菜单](#dropdown-下拉菜单)
  - [Steps 步骤条](#steps-步骤条)
- [Others 其他](#others-其他)
  - [Dialog 对话框](#dialog-对话框)
  - [Tooltip 文字提示](#tooltip-文字提示)
  - [Popover 弹出框](#popover-弹出框)
  - [Popconfirm 气泡确认框](#popconfirm-气泡确认框)
  - [Card 卡片](#card-卡片)
  - [Carousel 走马灯](#carousel-走马灯)
  - [Timeline 时间线](#timeline-时间线)
  - [Divider 分割线](#divider-分割线)
  - [Backtop 回到顶部](#backtop-回到顶部)
  - [Drawer 抽屉](#drawer-抽屉)
  - [Calendar 日历](#calendar-日历)

---

## Basic 基础组件

### Layout 布局

通过基础的 24 分栏，迅速简便地创建布局。

**核心属性（el-row）**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| gutter | 栅格间隔 | number | — | 0 |
| type | 布局模式 | string | flex | — |
| justify | flex 布局下的水平排列方式 | string | start/end/center/space-around/space-between | start |
| align | flex 布局下的垂直排列方式 | string | top/middle/bottom | top |
| tag | 自定义元素标签 | string | — | div |

**核心属性（el-col）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| span | 栅格占据的列数 | number | 24 |
| offset | 栅格左侧的间隔格数 | number | 0 |
| push | 栅格向右移动格数 | number | 0 |
| pull | 栅格向左移动格数 | number | 0 |
| xs/sm/md/lg/xl | 响应式栅格 | number/object | — |

**示例**

```html
<el-row :gutter="20">
  <el-col :span="12"><div>左</div></el-col>
  <el-col :span="12"><div>右</div></el-col>
</el-row>
```

官网：https://element.eleme.cn/#/zh-CN/component/layout

---

### Container 布局容器

用于布局的容器组件，方便快速搭建页面的基本结构。

**组件**

| 组件 | 说明 |
|------|------|
| el-container | 外层容器，可设置 direction |
| el-header | 顶栏容器 |
| el-aside | 侧边栏容器 |
| el-main | 主要区域容器 |
| el-footer | 底栏容器 |

**核心属性（el-container）**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| direction | 子元素排列方向 | string | horizontal/vertical | 子元素中有 el-header 或 el-footer 时为 vertical，否则为 horizontal |

**示例**

```html
<el-container>
  <el-aside width="200px">Aside</el-aside>
  <el-container>
    <el-header>Header</el-header>
    <el-main>Main</el-main>
    <el-footer>Footer</el-footer>
  </el-container>
</el-container>
```

官网：https://element.eleme.cn/#/zh-CN/component/container

---

### Button 按钮

**核心属性**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| type | 类型 | string | primary/success/warning/danger/info/text | — |
| size | 尺寸 | string | medium/small/mini | — |
| plain | 朴素按钮 | boolean | — | false |
| round | 圆角按钮 | boolean | — | false |
| circle | 圆形按钮 | boolean | — | false |
| loading | 加载中状态 | boolean | — | false |
| disabled | 禁用 | boolean | — | false |
| icon | 图标类名 | string | — | — |
| native-type | 原生 type 属性 | string | button/submit/reset | button |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| click | 点击事件 | — |

**示例**

```html
<el-button type="primary" @click="handleClick">主要按钮</el-button>
<el-button type="success" icon="el-icon-check" :loading="submitting">提交</el-button>
<el-button type="text">文字按钮</el-button>
```

官网：https://element.eleme.cn/#/zh-CN/component/button

---

### Link 文字链接

**核心属性**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| type | 类型 | string | primary/success/warning/danger/info | default |
| underline | 是否下划线 | boolean | — | true |
| disabled | 禁用 | boolean | — | false |
| href | 原生 href | string | — | — |
| icon | 图标类名 | string | — | — |

**示例**

```html
<el-link type="primary" href="https://element.eleme.cn">主要链接</el-link>
```

官网：https://element.eleme.cn/#/zh-CN/component/link

---

### Icon 图标

Element UI 自带一套常用图标。

**使用方式**

```html
<i class="el-icon-edit"></i>
<i class="el-icon-share"></i>
<i class="el-icon-delete"></i>
<i class="el-icon-search"></i>
```

常用图标：el-icon-edit, el-icon-delete, el-icon-search, el-icon-plus, el-icon-minus, el-icon-check, el-icon-close, el-icon-upload, el-icon-download, el-icon-refresh, el-icon-view, el-icon-setting, el-icon-user, el-icon-date, el-icon-loading。

官网：https://element.eleme.cn/#/zh-CN/component/icon

---

## Form 表单组件

### Radio 单选框

**核心属性（el-radio）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | string/number/boolean | — |
| label | Radio 的 value | string/number/boolean | — |
| disabled | 禁用 | boolean | false |
| border | 边框 | boolean | false |
| size | 尺寸 | string | — |

**核心属性（el-radio-group）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | string/number/boolean | — |
| size | 尺寸 | string | — |
| disabled | 禁用 | boolean | false |
| text-color | 按钮形式激活时的文本颜色 | string | #ffffff |
| fill | 按钮形式激活时的填充色 | string | #409EFF |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 绑定值变化时触发 | 选中的 label 值 |

**示例**

```html
<el-radio-group v-model="gender">
  <el-radio :label="1">男</el-radio>
  <el-radio :label="2">女</el-radio>
</el-radio-group>
```

官网：https://element.eleme.cn/#/zh-CN/component/radio

---

### Checkbox 多选框

**核心属性（el-checkbox）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | string/number/boolean | — |
| label | 选中状态的值 | string/number/boolean | — |
| disabled | 禁用 | boolean | false |
| border | 边框 | boolean | false |
| checked | 当前是否勾选 | boolean | false |

**核心属性（el-checkbox-group）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | array | — |
| disabled | 禁用 | boolean | false |
| min | 可被勾选的最小数量 | number | — |
| max | 可被勾选的最大数量 | number | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 绑定值变化时触发 | 新的值数组 |

**示例**

```html
<el-checkbox-group v-model="hobbies">
  <el-checkbox label="阅读"></el-checkbox>
  <el-checkbox label="运动"></el-checkbox>
  <el-checkbox label="音乐"></el-checkbox>
</el-checkbox-group>
```

官网：https://element.eleme.cn/#/zh-CN/component/checkbox

---

### Input 输入框

**核心属性**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| type | 类型 | string | text/textarea/password/number 等 | text |
| value / v-model | 绑定值 | string/number | — | — |
| maxlength | 最大输入长度 | number | — | — |
| minlength | 最小输入长度 | number | — | — |
| show-word-limit | 显示字数统计 | boolean | — | false |
| placeholder | 占位文本 | string | — | — |
| clearable | 可清空 | boolean | — | false |
| show-password | 显示切换密码 | boolean | — | false |
| disabled | 禁用 | boolean | — | false |
| size | 尺寸 | string | medium/small/mini | — |
| prefix-icon | 头部图标 | string | — | — |
| suffix-icon | 尾部图标 | string | — | — |
| rows | 行数（textarea） | number | — | 2 |
| autosize | 自适应内容高度 | boolean/object | — | false |
| readonly | 只读 | boolean | — | false |
| resize | 缩放图标 | string | none/both/horizontal/vertical | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| blur | 失去焦点时触发 | event |
| focus | 获得焦点时触发 | event |
| change | 内容变化时触发 | value |
| input | 输入时触发 | value |
| clear | 清空时触发 | — |

**方法**

| 方法名 | 说明 |
|--------|------|
| focus | 使 input 获取焦点 |
| blur | 使 input 失去焦点 |
| select | 选中 input 中的文字 |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| prefix | 输入框头部内容 |
| suffix | 输入框尾部内容 |
| prepend | 输入框前置内容 |
| append | 输入框后置内容 |

**示例**

```html
<el-input v-model="name" placeholder="请输入名称" clearable></el-input>
<el-input type="textarea" v-model="remark" :rows="3" placeholder="请输入备注"></el-input>
<el-input placeholder="搜索" prefix-icon="el-icon-search" v-model="keyword"></el-input>
```

官网：https://element.eleme.cn/#/zh-CN/component/input

---

### InputNumber 计数器

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | number | — |
| min | 最小值 | number | -Infinity |
| max | 最大值 | number | Infinity |
| step | 步长 | number | 1 |
| step-strictly | 严格步进 | boolean | false |
| precision | 精度（小数位数） | number | — |
| disabled | 禁用 | boolean | false |
| controls | 是否显示控制按钮 | boolean | true |
| controls-position | 控制按钮位置 | string | right |
| size | 尺寸 | string | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | currentValue, oldValue |
| blur | 失焦 | event |
| focus | 聚焦 | event |

**示例**

```html
<el-input-number v-model="num" :min="1" :max="100" :step="1"></el-input-number>
```

官网：https://element.eleme.cn/#/zh-CN/component/input-number

---

### Select 选择器

**核心属性（el-select）**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| value / v-model | 绑定值 | string/number/boolean/object | — | — |
| multiple | 多选 | boolean | — | false |
| disabled | 禁用 | boolean | — | false |
| clearable | 可清空 | boolean | — | false |
| filterable | 可搜索 | boolean | — | false |
| allow-create | 允许创建条目 | boolean | — | false |
| loading | 加载中 | boolean | — | false |
| remote | 是否为远程搜索 | boolean | — | false |
| remote-method | 远程搜索方法 | function | — | — |
| placeholder | 占位文本 | string | — | — |
| size | 尺寸 | string | medium/small/mini | — |
| collapse-tags | 多选时是否折叠 tag | boolean | — | false |
| multiple-limit | 多选最大数量 | number | — | 0 |

**核心属性（el-option）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value | 选项的值 | string/number/object | — |
| label | 选项的标签 | string | — |
| disabled | 禁用 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | 当前 value |
| visible-change | 下拉框出现/隐藏时 | true/false |
| remove-tag | 多选模式下移除 tag 时 | tag 值 |
| clear | 可清空模式下清空时 | — |
| blur | 失焦 | event |
| focus | 聚焦 | event |

**方法**

| 方法名 | 说明 |
|--------|------|
| focus | 聚焦 |
| blur | 失焦 |

**示例**

```html
<!-- 基础用法 -->
<el-select v-model="status" placeholder="请选择" clearable>
  <el-option label="启用" :value="1"></el-option>
  <el-option label="禁用" :value="0"></el-option>
</el-select>

<!-- 多选 -->
<el-select v-model="tags" multiple placeholder="请选择标签">
  <el-option label="标签1" value="tag1"></el-option>
  <el-option label="标签2" value="tag2"></el-option>
</el-select>

<!-- 可搜索 -->
<el-select v-model="user" filterable placeholder="请搜索选择">
  <el-option v-for="item in userList" :key="item.id" :label="item.name" :value="item.id"></el-option>
</el-select>

<!-- 远程搜索 -->
<el-select v-model="remoteValue" filterable remote :remote-method="remoteSearch" :loading="searchLoading" placeholder="请输入关键词">
  <el-option v-for="item in remoteOptions" :key="item.id" :label="item.name" :value="item.id"></el-option>
</el-select>
```

官网：https://element.eleme.cn/#/zh-CN/component/select

---

### Cascader 级联选择器

**核心属性**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| value / v-model | 绑定值 | array | — | — |
| options | 数据源 | array | — | — |
| props | 配置选项 | object | — | — |
| size | 尺寸 | string | medium/small/mini | — |
| placeholder | 占位文本 | string | — | 请选择 |
| disabled | 禁用 | boolean | — | false |
| clearable | 可清空 | boolean | — | false |
| show-all-levels | 显示完整路径 | boolean | — | true |
| collapse-tags | 多选时折叠 tag | boolean | — | false |
| separator | 路径分隔符 | string | — | / |
| filterable | 可搜索 | boolean | — | false |

**props 配置**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| expandTrigger | 展开触发方式 | string | click |
| multiple | 多选 | boolean | false |
| checkStrictly | 可选任意一级 | boolean | false |
| emitPath | 完整路径 | boolean | true |
| label | 指定标签 | string | label |
| value | 指定值 | string | value |
| children | 指定子项 | string | children |
| leaf | 指定叶节点 | string | leaf |
| lazy | 懒加载 | boolean | false |
| lazyLoad | 懒加载回调 | function | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | value |
| expand-change | 展开节点变化时 | value数组 |
| blur | 失焦 | event |
| focus | 聚焦 | event |

**方法**

| 方法名 | 说明 |
|--------|------|
| getCheckedNodes | 获取选中的节点 |

**示例**

```html
<!-- 基础用法 -->
<el-cascader v-model="region" :options="regionOptions" :props="{ value: 'id', label: 'name', children: 'children' }" clearable></el-cascader>

<!-- 可选任意一级 -->
<el-cascader v-model="category" :options="categoryOptions" :props="{ checkStrictly: true }" clearable></el-cascader>

<!-- 懒加载 -->
<el-cascader v-model="lazyValue" :props="{ lazy: true, lazyLoad: lazyLoadMethod }"></el-cascader>
```

官网：https://element.eleme.cn/#/zh-CN/component/cascader

---

### Switch 开关

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | boolean/string/number | — |
| disabled | 禁用 | boolean | false |
| active-color | 开启时颜色 | string | #409EFF |
| inactive-color | 关闭时颜色 | string | #C0CCDA |
| active-text | 开启文字 | string | — |
| inactive-text | 关闭文字 | string | — |
| active-value | 开启对应的值 | boolean/string/number | true |
| inactive-value | 关闭对应的值 | boolean/string/number | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 开关状态变化时 | 新的值 |

**示例**

```html
<el-switch v-model="enabled" active-text="启用" inactive-text="禁用"></el-switch>
<el-switch v-model="status" :active-value="1" :inactive-value="0"></el-switch>
```

官网：https://element.eleme.cn/#/zh-CN/component/switch

---

### Slider 滑块

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | number | 0 |
| min | 最小值 | number | 0 |
| max | 最大值 | number | 100 |
| step | 步长 | number | 1 |
| show-input | 显示输入框 | boolean | false |
| show-input-controls | 显示输入框控制按钮 | boolean | true |
| show-stops | 显示间断点 | boolean | false |
| show-tooltip | 显示 tooltip | boolean | true |
| disabled | 禁用 | boolean | false |
| range | 范围选择 | boolean | false |
| vertical | 垂直 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化（松开时） | 新值 |
| input | 拖动时 | 新值 |

**示例**

```html
<el-slider v-model="progress"></el-slider>
<el-slider v-model="range" range :max="100"></el-slider>
```

官网：https://element.eleme.cn/#/zh-CN/component/slider

---

### TimePicker 时间选择器

**核心属性（el-time-picker）**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| value / v-model | 绑定值 | string/array | — | — |
| is-range | 是否范围选择 | boolean | — | false |
| placeholder | 占位 | string | — | — |
| start-placeholder | 范围开始占位 | string | — | — |
| end-placeholder | 范围结束占位 | string | — | — |
| format | 时间格式 | string | — | HH:mm:ss |
| value-format | 可选时间格式 | string | — | — |
| disabled | 禁用 | boolean | — | false |
| clearable | 可清空 | boolean | — | true |
| picker-options | 当前时间选择器特有选项 | object | — | — |

**picker-options**

| 参数 | 说明 | 类型 |
|------|------|------|
| selectableRange | 可选时间段 | string/array |
| format | 时间格式化 | string |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | 新值 |
| blur | 失焦 | event |
| focus | 聚焦 | event |

**示例**

```html
<el-time-picker v-model="time" placeholder="选择时间" format="HH:mm:ss"></el-time-picker>
<el-time-picker is-range v-model="timeRange" range-separator="至" start-placeholder="开始时间" end-placeholder="结束时间"></el-time-picker>
```

官网：https://element.eleme.cn/#/zh-CN/component/time-picker

---

### DatePicker 日期选择器

**核心属性**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| value / v-model | 绑定值 | string/array/date | — | — |
| type | 类型 | string | date/datetime/year/month/week/dates/daterange/datetimerange/dateranges | date |
| placeholder | 占位 | string | — | — |
| start-placeholder | 范围开始占位 | string | — | — |
| end-placeholder | 范围结束占位 | string | — | — |
| format | 显示格式 | string | — | yyyy-MM-dd |
| value-format | 可选绑定值格式 | string | — | — |
| disabled | 禁用 | boolean | — | false |
| clearable | 可清空 | boolean | — | true |
| editable | 文本框可输入 | boolean | — | true |
| picker-options | 选项 | object | — | — |

**picker-options**

| 参数 | 说明 | 类型 |
|------|------|------|
| disabledDate | 禁用日期 | function |
| shortcuts | 快捷选项 | object[] |
| firstDayOfWeek | 周起始日 | number |
| onPick | 选中日期后会执行 | function |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | 新值 |
| blur | 失焦 | event |
| focus | 聚焦 | event |

**示例**

```html
<!-- 基础日期 -->
<el-date-picker v-model="date" type="date" placeholder="选择日期" value-format="yyyy-MM-dd"></el-date-picker>

<!-- 日期范围 -->
<el-date-picker v-model="dateRange" type="daterange" range-separator="至" start-placeholder="开始日期" end-placeholder="结束日期" value-format="yyyy-MM-dd"></el-date-picker>

<!-- 日期时间 -->
<el-date-picker v-model="datetime" type="datetime" placeholder="选择日期时间" value-format="yyyy-MM-dd HH:mm:ss"></el-date-picker>

<!-- 禁用日期 -->
<el-date-picker v-model="date" type="date" :picker-options="pickerOptions" placeholder="选择日期"></el-date-picker>
```

```javascript
data() {
  return {
    pickerOptions: {
      disabledDate(time) {
        // 禁用今天之前的日期
        return time.getTime() < Date.now() - 8.64e7
      },
      shortcuts: [{
        text: '今天',
        onClick(picker) {
          picker.$emit('pick', new Date())
        }
      }, {
        text: '最近一周',
        onClick(picker) {
          const date = new Date()
          date.setTime(date.getTime() - 3600 * 1000 * 24 * 7)
          picker.$emit('pick', date)
        }
      }]
    }
  }
}
```

官网：https://element.eleme.cn/#/zh-CN/component/date-picker

---

### DateTimePicker 日期时间选择器

实际上 el-date-picker 的 type=datetime 就是日期时间选择器，文档中将 DateTimePicker 单列为组件，但用法与 DatePicker 基本一致。

**核心属性**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| value / v-model | 绑定值 | string/date | — | — |
| placeholder | 占位 | string | — | — |
| type | 固定 | string | — | datetime |
| format | 显示格式 | string | — | yyyy-MM-dd HH:mm:ss |
| value-format | 可选绑定值格式 | string | — | — |
| default-time | 默认时间 | string | — | — |
| picker-options | 选项 | object | — | — |

**示例**

```html
<el-date-picker v-model="datetime" type="datetime" placeholder="选择日期时间" value-format="yyyy-MM-dd HH:mm:ss" default-time="12:00:00"></el-date-picker>
```

官网：https://element.eleme.cn/#/zh-CN/component/datetime-picker

---

### Upload 上传

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| action | 上传地址（必填） | string | — |
| headers | 请求头 | object | — |
| multiple | 多选 | boolean | false |
| data | 附带参数 | object | — |
| name | 文件字段名 | string | file |
| with-credentials | 支持发送 cookie | boolean | false |
| show-file-list | 显示文件列表 | boolean | true |
| drag | 拖拽上传 | boolean | false |
| accept | 接受的文件类型 | string | — |
| list-type | 文件列表样式 | string | text/picture/picture-card |
| auto-upload | 自动上传 | boolean | true |
| limit | 最大数量 | number | — |
| file-list | 文件列表 | array | — |
| disabled | 禁用 | boolean | false |
| http-request | 覆盖默认上传 | function | — |
| before-upload | 上传前钩子 | function | — |
| before-remove | 删除前钩子 | function | — |
| on-preview | 点击文件列表钩子 | function | — |
| on-remove | 删除文件钩子 | function | — |
| on-success | 成功钩子 | function | — |
| on-error | 失败钩子 | function | — |
| on-progress | 进度钩子 | function | — |
| on-change | 文件状态变化钩子 | function | — |
| on-exceed | 超出数量限制钩子 | function | — |

**方法**

| 方法名 | 说明 |
|--------|------|
| clearFiles | 清空已上传的文件列表 |
| abort | 取消上传请求 |
| submit | 手动上传 |
| handleStart | 手动选择文件 |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| default | 触发区/拖拽区内容 |
| trigger | 触发上传的区域 |
| tip | 提示信息区域 |

**示例**

```html
<!-- 点击上传 -->
<el-upload action="/api/upload" :on-success="handleSuccess" :before-upload="beforeUpload">
  <el-button size="small" type="primary">点击上传</el-button>
  <div slot="tip" class="el-upload__tip">只能上传jpg/png文件，且不超过500kb</div>
</el-upload>

<!-- 照片墙 -->
<el-upload action="/api/upload" list-type="picture-card" :on-preview="handlePreview" :on-remove="handleRemove">
  <i class="el-icon-plus"></i>
</el-upload>

<!-- 拖拽上传 -->
<el-upload drag action="/api/upload" :on-success="handleSuccess" multiple>
  <i class="el-icon-upload"></i>
  <div class="el-upload__text">将文件拖到此处，或<em>点击上传</em></div>
</el-upload>

<!-- 手动上传 -->
<el-upload action="/api/upload" :auto-upload="false" :on-success="handleSuccess" ref="upload">
  <el-button slot="trigger" size="small" type="primary">选取文件</el-button>
  <el-button size="small" type="success" @click="submitUpload">上传到服务器</el-button>
</el-upload>
```

官网：https://element.eleme.cn/#/zh-CN/component/upload

---

### Rate 评分

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | number | 0 |
| max | 最大分值 | number | 5 |
| disabled | 禁用 | boolean | false |
| allow-half | 允许半选 | boolean | false |
| show-score | 显示分数 | boolean | false |
| show-text | 显示文字 | boolean | false |
| texts | 辅助文字数组 | array | — |
| colors | 颜色分段 | array | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | 新值 |

**示例**

```html
<el-rate v-model="score" show-text></el-rate>
<el-rate v-model="score" :colors="['#99A9BF', '#F7BA2A', '#FF9900']" :max="10" allow-half></el-rate>
```

官网：https://element.eleme.cn/#/zh-CN/component/rate

---

### ColorPicker 颜色选择器

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | string | — |
| disabled | 禁用 | boolean | false |
| show-alpha | 支持 alpha 通道 | boolean | false |
| color-format | 颜色格式 | string | hsl/hex/rgb | — |
| predefine | 预定义颜色 | array | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | 新值 |
| active-change | 面板中当前颜色变化 | 当前色彩值 |

**示例**

```html
<el-color-picker v-model="color"></el-color-picker>
<el-color-picker v-model="color" show-alpha :predefine="predefineColors"></el-color-picker>
```

官网：https://element.eleme.cn/#/zh-CN/component/color-picker

---

### Transfer 穿梭框

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | array | — |
| data | 数据源 | array | — |
| titles | 自定义列表标题 | array | ['列表1', '列表2'] |
| filterable | 可搜索 | boolean | false |
| filter-placeholder | 搜索占位 | string | 请输入搜索内容 |
| filter-method | 自定义搜索 | function | — |
| target-order | 右侧排序 | string | original/push/unshift | original |
| props | 数据源字段映射 | object | — |
| left-default-checked | 左侧默认选中 | array | — |
| right-default-checked | 右侧默认选中 | array | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 值变化时 | 当前值, 数据移动方向, 移动的数据 |
| left-check-change | 左侧勾选变化 | 当前勾选的 key |
| right-check-change | 右侧勾选变化 | 当前勾选的 key |

**示例**

```html
<el-transfer v-model="checkedList" :data="transferData" :titles="['可选', '已选']" filterable></el-transfer>
```

官网：https://element.eleme.cn/#/zh-CN/component/transfer

---

### Tag 标签

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| type | 类型 | string | success/info/warning/danger | — |
| closable | 可关闭 | boolean | false |
| disable-transitions | 禁用动画 | boolean | false |
| hit | 边框 | boolean | false |
| color | 背景色 | string | — |
| size | 尺寸 | string | medium/small/mini | — |
| effect | 主题 | string | dark/light/plain | light |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| click | 点击 | — |
| close | 关闭 | — |

**示例**

```html
<el-tag>默认标签</el-tag>
<el-tag type="success">成功标签</el-tag>
<el-tag type="warning" closable @close="handleClose">可关闭标签</el-tag>
<el-tag v-for="tag in tags" :key="tag" closable @close="removeTag(tag)">{{ tag }}</el-tag>
```

官网：https://element.eleme.cn/#/zh-CN/component/tag

---

### Form 表单

**核心属性（el-form）**

| 参数 | 说明 | 类型 | 可选值 | 默认值 |
|------|------|------|--------|--------|
| model | 表单数据对象 | object | — | — |
| rules | 验证规则 | object | — | — |
| inline | 行内表单 | boolean | — | false |
| label-position | 标签位置 | string | left/right/top | right |
| label-width | 标签宽度 | string | — | — |
| label-suffix | 标签后缀 | string | — | — |
| show-message | 显示校验信息 | boolean | — | true |
| inline-message | 行内校验信息 | boolean | — | false |
| status-icon | 校验反馈图标 | boolean | — | false |
| size | 尺寸 | string | medium/small/mini | — |
| disabled | 禁用 | boolean | — | false |
| hide-required-asterisk | 隐藏必填星号 | boolean | — | false |

**核心属性（el-form-item）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| prop | 模型字段 | string | — |
| label | 标签文本 | string | — |
| label-width | 标签宽度 | string | — |
| required | 必填 | boolean | false |
| rules | 校验规则 | object/array | — |
| error | 错误信息 | string | — |
| show-message | 显示校验信息 | boolean | true |
| inline-message | 行内信息 | boolean | false |

**方法**

| 方法名 | 说明 | 参数 |
|--------|------|------|
| validate | 对整个表单校验 | callback |
| validateField | 校验某个字段 | prop, callback |
| resetFields | 重置字段 | props |
| clearValidate | 清除校验 | props |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| validate | 任一表单项被校验后触发 | prop, valid, message |

**示例**

```html
<el-form :model="form" :rules="rules" ref="form" label-width="100px">
  <el-form-item label="名称" prop="name">
    <el-input v-model="form.name" placeholder="请输入名称"></el-input>
  </el-form-item>
  <el-form-item label="状态" prop="status">
    <el-select v-model="form.status" placeholder="请选择状态">
      <el-option label="启用" :value="1"></el-option>
      <el-option label="禁用" :value="0"></el-option>
    </el-select>
  </el-form-item>
  <el-form-item>
    <el-button type="primary" @click="submitForm('form')">提交</el-button>
    <el-button @click="resetForm('form')">重置</el-button>
  </el-form-item>
</el-form>
```

```javascript
export default {
  data() {
    return {
      form: {
        name: '',
        status: ''
      },
      rules: {
        name: [
          { required: true, message: '请输入名称', trigger: 'blur' },
          { min: 2, max: 20, message: '长度在 2 到 20 个字符', trigger: 'blur' }
        ],
        status: [
          { required: true, message: '请选择状态', trigger: 'change' }
        ]
      }
    }
  },
  methods: {
    submitForm(formName) {
      this.$refs[formName].validate((valid) => {
        if (valid) {
          // 校验通过，提交表单
          this.submitData()
        } else {
          this.$message.error('请完善表单信息')
          return false
        }
      })
    },
    resetForm(formName) {
      this.$refs[formName].resetFields()
    }
  }
}
```

官网：https://element.eleme.cn/#/zh-CN/component/form

---

## Data 数据展示

### Table 表格

**核心属性（el-table）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| data | 数据源 | array | — |
| height | 高度 | string/number | — |
| max-height | 最大高度 | string/number | — |
| border | 纵向边框 | boolean | false |
| stripe | 斑马纹 | boolean | false |
| row-key | 行数据的 Key | function/string | — |
| highlight-current-row | 当前行高亮 | boolean | false |
| default-expand-all | 默认展开所有 | boolean | false |
| tree-props | 树形配置 | object | — |
| span-method | 合并行/列方法 | function | — |
| row-class-name | 行 className | function | — |
| cell-class-name | 单元格 className | function | — |
| header-row-class-name | 表头行 className | function | — |
| header-cell-class-name | 表头单元格 className | function | — |
| empty-text | 空数据文本 | string | 暂无数据 |
| show-summary | 合计行 | boolean | false |
| sum-text | 合计行文本 | string | 合计 |
| summary-method | 自定义合计方法 | function | — |

**核心属性（el-table-column）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| type | 列类型 | string | selection/index/expand | — |
| label | 标题 | string | — |
| prop | 字段名 | string | — |
| width | 列宽 | string | — |
| min-width | 最小列宽 | string | — |
| fixed | 固定列 | string/boolean | true/left/right | — |
| sortable | 排序 | boolean/string | true/false/custom | false |
| sort-method | 自定义排序 | function | — |
| sort-by | 排序依据 | string/array/function | — |
| align | 对齐 | string | left/center/right | left |
| header-align | 表头对齐 | string | — |
| show-overflow-tooltip | 超出省略 | boolean | false |
| formatter | 格式化 | function | — |
| selectable | 是否可选 | function | — |
| reserve-selection | 保留选择 | boolean | false |
| filters | 过滤器 | array | — |
| filter-method | 过滤方法 | function | — |
| filtered-value | 过滤默认值 | array | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| select | 选择行触发 | selection, row |
| select-all | 全选触发 | selection |
| selection-change | 选择变化 | selection |
| cell-click | 单元格点击 | row, column, cell, event |
| row-click | 行点击 | row, column, event |
| row-dblclick | 行双击 | row, column, event |
| sort-change | 排序变化 | { column, prop, order } |
| filter-change | 过滤变化 | filters |
| current-change | 当前行变化 | currentRow, oldCurrentRow |
| header-click | 表头点击 | column, event |
| expand-change | 展开行变化 | row, expandedRows |

**方法**

| 方法名 | 说明 | 参数 |
|--------|------|------|
| clearSelection | 清空选择 | — |
| toggleRowSelection | 切换选中 | row, selected |
| toggleAllSelection | 全选切换 | — |
| toggleRowExpansion | 切换展开 | row, expanded |
| setCurrentRow | 设置当前行 | row |
| clearSort | 清空排序 | — |
| clearFilter | 清空过滤 | columnKey |
| doLayout | 重新布局 | — |
| sort | 排序 | prop, order |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| default | 自定义列内容 |
| append | 表格末尾追加 |
| empty | 空数据 |
| expand | 展开行内容 |

**scoped slot 参数**

```
{ row, column, $index, store, _self }
```

**示例**

```html
<!-- 基础表格 -->
<el-table :data="tableData" border style="width: 100%" v-loading="loading">
  <el-table-column type="selection" width="55"></el-table-column>
  <el-table-column type="index" label="序号" width="50"></el-table-column>
  <el-table-column prop="name" label="名称"></el-table-column>
  <el-table-column prop="status" label="状态">
    <template slot-scope="scope">
      <el-tag :type="scope.row.status === 1 ? 'success' : 'info'">
        {{ scope.row.status === 1 ? '启用' : '禁用' }}
      </el-tag>
    </template>
  </el-table-column>
  <el-table-column label="操作" width="180">
    <template slot-scope="scope">
      <el-button size="mini" type="text" @click="handleEdit(scope.row)">编辑</el-button>
      <el-button size="mini" type="text" style="color: #F56C6C" @click="handleDelete(scope.row)">删除</el-button>
    </template>
  </el-table-column>
</el-table>
```

官网：https://element.eleme.cn/#/zh-CN/component/table

---

### Progress 进度条

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| percentage | 百分比 | number | 0 |
| type | 类型 | string | line/circle/dashboard | line |
| stroke-width | 线宽 | number | 6 |
| color | 颜色 | string/function | — |
| width | 圆形宽度 | number | 126 |
| show-text | 显示文字 | boolean | true |
| text-inside | 文字内置 | boolean | false |
| status | 状态 | string | success/exception/warning | — |
| format | 格式化 | function | — |

**示例**

```html
<el-progress :percentage="50"></el-progress>
<el-progress :percentage="100" status="success"></el-progress>
<el-progress :percentage="70" :color="customColor"></el-progress>
<el-progress type="circle" :percentage="75"></el-progress>
```

官网：https://element.eleme.cn/#/zh-CN/component/progress

---

### Tree 树形控件

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| data | 数据源 | array | — |
| props | 配置 | object | — |
| node-key | 节点 key | string | — |
| default-expanded-keys | 默认展开 | array | — |
| default-checked-keys | 默认勾选 | array | — |
| show-checkbox | 显示勾选框 | boolean | false |
| check-strictly | 父子不关联 | boolean | false |
| accordion | 手风琴模式 | boolean | false |
| highlight-current | 高亮当前 | boolean | false |
| current-node-key | 当前节点 key | string/number | — |
| expand-on-click-node | 点击展开 | boolean | true |
| filter-node-method | 过滤方法 | function | — |
| render-content | 渲染内容 | function | — |
| lazy | 懒加载 | boolean | false |
| load | 加载方法 | function | — |
| draggable | 可拖拽 | boolean | false |

**props 配置**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| label | 标签 | string/function | label |
| children | 子项 | string | children |
| disabled | 禁用 | string/function | disabled |
| isLeaf | 叶节点 | string/function | isLeaf |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| node-click | 节点点击 | data, node, self |
| node-contextmenu | 右键 | event, data, node, self |
| check-change | 勾选变化 | data, checked, indeterminate |
| check | 勾选 | data, { checkedNodes, checkedKeys, halfCheckedNodes, halfCheckedKeys } |
| current-change | 当前变化 | data, node |
| node-expand | 展开 | data, node, self |
| node-collapse | 收起 | data, node, self |
| node-drag-start | 拖拽开始 | node, event |
| node-drag-end | 拖拽结束 | event, node, node2, location |
| node-drop | 放置 | node, node2, location |

**方法**

| 方法名 | 说明 |
|--------|------|
| setCurrentKey | 设置当前节点 |
| getCurrentKey | 获取当前 key |
| getCurrentNode | 获取当前节点 |
| setCheckedKeys | 设置勾选 |
| getCheckedKeys | 获取勾选 key |
| setCheckedNodes | 设置勾选节点 |
| getCheckedNodes | 获取勾选节点 |
| setChecked | 设置勾选 |
| getCheckedNodes | 获取勾选 |
| getHalfCheckedNodes | 获取半选节点 |
| getHalfCheckedKeys | 获取半选 key |
| append | 追加 |
| remove | 删除 |
| insert | 插入 |
| filter | 过滤 |
| updateKeyChildren | 更新子节点 |

**示例**

```html
<el-tree :data="treeData" :props="defaultProps" @node-click="handleNodeClick" show-checkbox></el-tree>
```

官网：https://element.eleme.cn/#/zh-CN/component/tree

---

### Pagination 分页

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| total | 总条数 | number | — |
| page-size | 每页条数 | number | — |
| current-page | 当前页 | number | — |
| page-sizes | 可选每页条数 | number[] | — |
| layout | 布局 | string | — |
| background | 背景色 | boolean | false |
| small | 小型 | boolean | false |
| pager-count | 页码按钮数 | number | 7 |
| prev-text | 上一页文字 | string | — |
| next-text | 下一页文字 | string | — |
| hide-on-single-page | 单页隐藏 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| size-change | 每页条数变化 | size |
| current-change | 当前页变化 | currentPage |
| prev-click | 上一页 | currentPage |
| next-click | 下一页 | currentPage |

**示例**

```html
<el-pagination
  @size-change="handleSizeChange"
  @current-change="handleCurrentChange"
  :current-page="currentPage"
  :page-sizes="[10, 20, 50, 100]"
  :page-size="pageSize"
  :total="total"
  layout="total, sizes, prev, pager, next, jumper"
  background
></el-pagination>
```

官网：https://element.eleme.cn/#/zh-CN/component/pagination

---

### Badge 标记

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value | 显示值 | string/number | — |
| max | 最大值 | number | — |
| is-dot | 小圆点 | boolean | false |
| hidden | 隐藏 | boolean | false |
| type | 类型 | string | primary/success/warning/danger/info | primary |

**示例**

```html
<el-badge :value="12" :max="99">
  <el-button>消息</el-button>
</el-badge>
<el-badge is-dot>
  <el-button>通知</el-button>
</el-badge>
```

官网：https://element.eleme.cn/#/zh-CN/component/badge

---

### Avatar 头像

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| src | 图片地址 | string | — |
| size | 尺寸 | number/string | large/medium/small | large |
| shape | 形状 | string | circle/square | circle |
| icon | 图标 | string | — |
| fit | 适应方式 | string | fill/contain/cover/none/scale-down | cover |
| error | 图片加载失败 | function | — |

**示例**

```html
<el-avatar src="https://example.com/avatar.jpg"></el-avatar>
<el-avatar icon="el-icon-user-solid"></el-avatar>
<el-avatar size="small" shape="square">{{ name.substring(0,1) }}</el-avatar>
```

官网：https://element.eleme.cn/#/zh-CN/component/avatar

---

### Skeleton 骨架屏

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| animated | 动画 | boolean | false |
| count | 渲染数量 | number | 1 |
| loading | 是否显示骨架 | boolean | true |
| rows | 行数 | number | 3 |
| throttle | 延迟 | number | 0 |
| default | 模板 | slot | — |
| template | 模板 | slot | — |

**示例**

```html
<el-skeleton :rows="5" animated />
<el-skeleton :loading="loading" animated>
  <div>
    <h3>{{ title }}</h3>
    <p>{{ content }}</p>
  </div>
</el-skeleton>
```

官网：https://element.eleme.cn/#/zh-CN/component/skeleton

---

### Empty 空状态

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| image | 图片地址 | string | — |
| image-size | 图片大小 | number | — |
| description | 描述 | string | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| image | 自定义图片 |
| description | 自定义描述 |
| default | 底部内容 |

**示例**

```html
<el-empty description="暂无数据"></el-empty>
<el-empty description="暂无数据">
  <el-button type="primary">添加数据</el-button>
</el-empty>
```

官网：https://element.eleme.cn/#/zh-CN/component/empty

---

### Image 图片

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| src | 图片地址 | string | — |
| fit | 适应方式 | string | — |
| lazy | 懒加载 | boolean | false |
| scroll-container | 滚动容器 | string/HTMLElement | — |
| preview-src-list | 预览图列表 | array | — |
| z-index | 预览层级 | number | 2000 |
| initial-index | 初始索引 | number | 0 |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| load | 加载成功 | — |
| error | 加载失败 | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| placeholder | 加载中占位 |
| error | 加载失败占位 |

**示例**

```html
<el-image src="https://example.com/img.jpg" fit="cover"></el-image>
<el-image :src="url" :preview-src-list="srcList"></el-image>
```

官网：https://element.eleme.cn/#/zh-CN/component/image

---

### Collapse 折叠面板

**核心属性（el-collapse）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 当前激活面板 | string/array | — |
| accordion | 手风琴模式 | boolean | false |

**核心属性（el-collapse-item）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| name | 唯一标识 | string/number | — |
| title | 标题 | string | — |
| disabled | 禁用 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 切换面板时 | activeNames |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| — | 默认（el-collapse-item 内容） |
| title | 自定义标题 |

**示例**

```html
<el-collapse v-model="activeNames" accordion>
  <el-collapse-item title="标题1" name="1">
    <div>内容1</div>
  </el-collapse-item>
  <el-collapse-item title="标题2" name="2">
    <div>内容2</div>
  </el-collapse-item>
</el-collapse>
```

官网：https://element.eleme.cn/#/zh-CN/component/collapse

---

## Notice 提示

### Alert 警告

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| type | 类型 | string | success/warning/info/error | info |
| title | 标题 | string | — |
| description | 描述 | string | — |
| closable | 可关闭 | boolean | true |
| show-icon | 显示图标 | boolean | false |
| center | 居中 | boolean | false |
| effect | 主题 | string | dark/light/plain | light |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| title | 标题 |
| — | 默认（描述） |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| close | 关闭时 | — |

**示例**

```html
<el-alert title="操作成功" type="success" show-icon></el-alert>
<el-alert title="警告信息" type="warning" description="这是一段警告描述" show-icon></el-alert>
<el-alert title="错误提示" type="error" show-icon :closable="false"></el-alert>
```

官网：https://element.eleme.cn/#/zh-CN/component/alert

---

### Loading 加载

**指令方式**

```html
<el-table v-loading="loading" element-loading-text="加载中..." element-loading-spinner="el-icon-loading"></el-table>
```

**服务方式**

```javascript
// 全局 loading
const loading = this.$loading({
  lock: true,
  text: 'Loading',
  spinner: 'el-icon-loading',
  background: 'rgba(0, 0, 0, 0.7)'
})
// 关闭
loading.close()
```

**属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| element-loading-text | 文本 | string | — |
| element-loading-spinner | 图标 | string | — |
| element-loading-background | 背景 | string | rgba(255,255,255,0.9) |
| element-loading-customClass | 自定义类 | string | — |
| element-loading-fullscreen | 全屏 | boolean | false |
| element-loading-lock | 锁定 | boolean | false |

官网：https://element.eleme.cn/#/zh-CN/component/loading

---

### Message 消息提示

**方法**

```javascript
this.$message('消息')
this.$message({
  message: '操作成功',
  type: 'success'
})
this.$message.error('错误消息')
this.$message.warning('警告消息')
this.$message.info('信息')
```

**参数**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| message | 消息内容 | string/VNode | — |
| type | 类型 | string | success/warning/info/error | info |
| duration | 显示时间 | number | 3000 |
| show-close | 可关闭 | boolean | false |
| center | 居中 | boolean | false |
| offset | 距顶部偏移 | number | 20 |
| custom-class | 自定义类 | string | — |
| dangerouslyUseHTMLString | HTML 渲染 | boolean | false |
| onClose | 关闭回调 | function | — |

官网：https://element.eleme.cn/#/zh-CN/component/message

---

### MessageBox 消息弹框

**方法**

```javascript
// 确认框
this.$confirm('确认删除该记录?', '提示', {
  confirmButtonText: '确定',
  cancelButtonText: '取消',
  type: 'warning'
}).then(() => {
  // 确认
  this.deleteRecord()
}).catch(() => {
  // 取消
})

// 提示框
this.$prompt('请输入名称', '提示', {
  confirmButtonText: '确定',
  cancelButtonText: '取消',
}).then(({ value }) => {
  // value 为输入值
})

// 消息框
this.$alert('这是一段内容', '标题', {
  confirmButtonText: '确定',
  callback: action => {}
})
```

**参数（confirm）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| title | 标题 | string | — |
| message | 消息 | string/VNode | — |
| type | 类型 | string | success/warning/info/error | — |
| confirmButtonText | 确认按钮文本 | string | 确定 |
| cancelButtonText | 取消按钮文本 | string | 取消 |
| confirmButtonClass | 确认按钮类 | string | — |
| cancelButtonClass | 取消按钮类 | string | — |
| showCancelButton | 显示取消 | boolean | false |
| showConfirmButton | 显示确认 | boolean | true |
| closeOnClickModal | 点击遮罩关闭 | boolean | true |
| closeOnPressEscape | ESC 关闭 | boolean | true |
| center | 居中 | boolean | false |
| dangerouslyUseHTMLString | HTML | boolean | false |
| beforeClose | 关闭前回调 | function | — |
| callback | 回调 | function | — |

官网：https://element.eleme.cn/#/zh-CN/component/message-box

---

### Notification 通知

**方法**

```javascript
this.$notify({
  title: '通知标题',
  message: '通知内容',
  type: 'success',
  duration: 4500
})

this.$notify.success('成功通知')
this.$notify.warning('警告通知')
this.$notify.error('错误通知')
```

**参数**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| title | 标题 | string | — |
| message | 消息 | string/VNode | — |
| type | 类型 | string | success/warning/info/error | — |
| duration | 显示时间 | number | 4500 |
| position | 位置 | string | top-right/top-left/bottom-right/bottom-left | top-right |
| show-close | 可关闭 | boolean | true |
| offset | 偏移 | number | 0 |
| custom-class | 自定义类 | string | — |
| dangerouslyUseHTMLString | HTML | boolean | false |
| onClose | 关闭回调 | function | — |
| onClick | 点击回调 | function | — |

官网：https://element.eleme.cn/#/zh-CN/component/notification

---

## Navigation 导航

### Menu 导航菜单

**核心属性（el-menu）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| default-active | 默认激活 | string | — |
| mode | 模式 | string | horizontal/vertical | vertical |
| collapse | 折叠 | boolean | false |
| background-color | 背景色 | string | #ffffff |
| text-color | 文字色 | string | #303133 |
| active-text-color | 激活色 | string | #409EFF |
| unique-opened | 只展开一个 | boolean | false |
| menu-trigger | 触发方式 | string | hover/click | hover |
| router | 使用路由 | boolean | false |

**核心属性（el-submenu / el-menu-item）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| index | 唯一标识 | string | — |
| route | 路由 | object | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| select | 菜单激活 | index, indexPath |
| open | submenu 展开 | index, indexPath |
| close | submenu 收起 | index, indexPath |

**示例**

```html
<el-menu :default-active="activeMenu" mode="horizontal" router>
  <el-menu-item index="/home">首页</el-menu-item>
  <el-submenu index="/system">
    <template slot="title">系统管理</template>
    <el-menu-item index="/system/user">用户管理</el-menu-item>
    <el-menu-item index="/system/role">角色管理</el-menu-item>
  </el-submenu>
</el-menu>
```

官网：https://element.eleme.cn/#/zh-CN/component/menu

---

### Tabs 标签页

**核心属性（el-tabs）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 当前激活 | string | — |
| type | 风格 | string | card/border-card | — |
| closable | 可关闭 | boolean | false |
| addable | 可增加 | boolean | false |
| editable | 可编辑 | boolean | false |
| tab-position | 位置 | string | top/right/bottom/left | top |
| stretch | 自适应宽度 | boolean | false |

**核心属性（el-tab-pane）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| label | 标题 | string | — |
| name | 标识 | string | — |
| disabled | 禁用 | boolean | false |
| lazy | 延迟渲染 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| tab-click | 点击 | pane |
| tab-remove | 移除 | name |
| tab-add | 增加 | — |
| edit | 编辑 | name, action |

**示例**

```html
<el-tabs v-model="activeTab" @tab-click="handleClick">
  <el-tab-pane label="用户管理" name="first">
    <user-management></user-management>
  </el-tab-pane>
  <el-tab-pane label="配置管理" name="second">
    <config-management></config-management>
  </el-tab-pane>
  <el-tab-pane label="角色管理" name="third">
    <role-management></role-management>
  </el-tab-pane>
</el-tabs>
```

官网：https://element.eleme.cn/#/zh-CN/component/tabs

---

### Breadcrumb 面包屑

**核心属性（el-breadcrumb）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| separator | 分隔符 | string | / |
| separator-class | 分隔符图标类 | string | — |

**核心属性（el-breadcrumb-item）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| to | 路由跳转 | string/object | — |
| replace | 替换历史 | boolean | false |

**示例**

```html
<el-breadcrumb separator="/">
  <el-breadcrumb-item :to="{ path: '/home' }">首页</el-breadcrumb-item>
  <el-breadcrumb-item :to="{ path: '/system' }">系统管理</el-breadcrumb-item>
  <el-breadcrumb-item>用户管理</el-breadcrumb-item>
</el-breadcrumb>
```

官网：https://element.eleme.cn/#/zh-CN/component/breadcrumb

---

### PageHeader 页头

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| title | 标题 | string | 返回 |
| content | 内容 | string | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| back | 点击返回时 | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| title | 自定义标题 |
| content | 自定义内容 |
| extra | 右侧额外内容 |

**示例**

```html
<el-page-header @back="goBack" content="用户详情"></el-page-header>
```

官网：https://element.eleme.cn/#/zh-CN/component/page-header

---

### Dropdown 下拉菜单

**核心属性（el-dropdown）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| type | 类型 | string | primary/info/success/warning/danger | — |
| size | 尺寸 | string | medium/small/mini | — |
| trigger | 触发方式 | string | hover/click | hover |
| placement | 出现位置 | string | top/top-start/top-end/bottom/bottom-start/bottom-end | bottom-end |
| disabled | 禁用 | boolean | false |
| split-button | 分裂按钮 | boolean | false |
| hide-on-click | 点击后隐藏 | boolean | true |
| show-timeout | 显示延迟 | number | 250 |
| hide-timeout | 隐藏延迟 | number | 150 |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| click | 点击触发按钮 | — |
| command | 点击菜单项 | command |
| visible-change | 显隐变化 | visible |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| default | 触发元素 |
| dropdown | 下拉菜单内容 |

**示例**

```html
<el-dropdown @command="handleCommand">
  <span class="el-dropdown-link">
    更多操作<i class="el-icon-arrow-down el-icon--right"></i>
  </span>
  <el-dropdown-menu slot="dropdown">
    <el-dropdown-item command="edit">编辑</el-dropdown-item>
    <el-dropdown-item command="delete" divided>删除</el-dropdown-item>
    <el-dropdown-item command="export" disabled>导出</el-dropdown-item>
  </el-dropdown-menu>
</el-dropdown>
```

官网：https://element.eleme.cn/#/zh-CN/component/dropdown

---

### Steps 步骤条

**核心属性（el-steps）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| active | 当前步骤 | number | 0 |
| process-status | 当前状态 | string | wait/process/finish/error/success | process |
| finish-status | 完成状态 | string | wait/process/finish/error/success | finish |
| align-center | 居中 | boolean | false |
| direction | 方向 | string | vertical/horizontal | horizontal |
| simple | 简洁风格 | boolean | false |

**核心属性（el-step）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| title | 标题 | string | — |
| description | 描述 | string | — |
| icon | 图标 | string | — |
| status | 状态 | string | wait/process/finish/error/success | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| icon | 自定义图标 |
| title | 自定义标题 |
| description | 自定义描述 |

**示例**

```html
<el-steps :active="activeStep" align-center finish-status="success">
  <el-step title="步骤1" description="填写信息"></el-step>
  <el-step title="步骤2" description="审核中"></el-step>
  <el-step title="步骤3" description="完成"></el-step>
</el-steps>
```

官网：https://element.eleme.cn/#/zh-CN/component/steps

---

## Others 其他

### Dialog 对话框

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| visible / .sync | 显示 | boolean | false |
| title | 标题 | string | — |
| width | 宽度 | string | 50% |
| fullscreen | 全屏 | boolean | false |
| top | 距顶距离 | string | 15vh |
| modal | 遮罩 | boolean | true |
| modal-append-to-body | 遮罩插入 body | boolean | true |
| append-to-body | 自身插入 body | boolean | false |
| lock-scroll | 锁定滚动 | boolean | true |
| custom-class | 自定义类 | string | — |
| close-on-click-modal | 点遮罩关闭 | boolean | true |
| close-on-press-escape | ESC 关闭 | boolean | true |
| show-close | 显示关闭 | boolean | true |
| before-close | 关闭前回调 | function | — |
| center | 居中 | boolean | false |
| destroy-on-close | 关闭销毁 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| open | 打开 | — |
| opened | 打开结束 | — |
| close | 关闭 | — |
| closed | 关闭结束 | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| — | 默认（内容） |
| title | 标题 |
| footer | 底部按钮 |

**示例**

```html
<el-dialog title="新增用户" :visible.sync="dialogVisible" width="600px" :before-close="handleClose">
  <el-form :model="form" :rules="rules" ref="form" label-width="100px">
    <el-form-item label="姓名" prop="name">
      <el-input v-model="form.name" placeholder="请输入姓名"></el-input>
    </el-form-item>
  </el-form>
  <span slot="footer">
    <el-button @click="dialogVisible = false">取 消</el-button>
    <el-button type="primary" @click="submitForm">确 定</el-button>
  </span>
</el-dialog>
```

官网：https://element.eleme.cn/#/zh-CN/component/dialog

---

### Tooltip 文字提示

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| content | 内容 | string | — |
| placement | 位置 | string | top/top-start/top-end/bottom/bottom-start/bottom-end/left/left-start/left-end/right/right-start/right-end | top |
| value / v-model | 显隐 | boolean | false |
| disabled | 禁用 | boolean | false |
| manual | 手动控制 | boolean | false |
| effect | 主题 | string | dark/light | dark |
| visible-arrow | 显示箭头 | boolean | true |
| transition | 动画 | string | el-fade-in-linear |
| open-delay | 显示延迟 | number | 0 |
| close-delay | 关闭延迟 | number | 0 |
| hide-after | 隐藏延迟 | number | 0 |
| enterable | 鼠标进入 | boolean | true |

**示例**

```html
<el-tooltip content="提示文字" placement="top">
  <el-button>悬停查看</el-button>
</el-tooltip>
```

官网：https://element.eleme.cn/#/zh-CN/component/tooltip

---

### Popover 弹出框

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| trigger | 触发方式 | string | click/focus/hover/manual | click |
| title | 标题 | string | — |
| content | 内容 | string | — |
| width | 宽度 | string/number | — |
| placement | 位置 | string | top | 
| disabled | 禁用 | boolean | false |
| v-model | 显隐 | boolean | false |
| transition | 动画 | string | el-fade-in-linear |
| visible-arrow | 箭头 | boolean | true |
| open-delay | 显示延迟 | number | 0 |
| close-delay | 关闭延迟 | number | 200 |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| — | 默认（内容） |

**示例**

```html
<el-popover trigger="click" title="标题" content="内容" width="200">
  <el-button>点击弹出</el-button>
</el-popover>
```

官网：https://element.eleme.cn/#/zh-CN/component/popover

---

### Popconfirm 气泡确认框

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| title | 标题 | string | — |
| confirm-button-text | 确认文本 | string | 确定 |
| cancel-button-text | 取消文本 | string | 取消 |
| confirm-button-type | 确认类型 | string | primary/success/warning/danger/info | primary |
| cancel-button-type | 取消类型 | string | text | — |
| icon | 图标 | string | el-icon-question |
| icon-color | 图标颜色 | string | #f90 |
| hide-icon | 隐藏图标 | boolean | false |
| hide-after | 隐藏延迟 | number | 200 |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| confirm | 确认 | — |
| cancel | 取消 | — |

**示例**

```html
<el-popconfirm title="确认删除吗？" @confirm="handleConfirm">
  <el-button slot="reference">删除</el-button>
</el-popconfirm>
```

官网：https://element.eleme.cn/#/zh-CN/component/popconfirm

---

### Card 卡片

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| header | 标题 | string | — |
| body-style | body 样式 | object | — |
| shadow | 阴影 | string | always/hover/never | always |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| header | 自定义标题 |
| — | 默认（内容） |

**示例**

```html
<el-card class="box-card" shadow="hover">
  <div slot="header">
    <span>卡片标题</span>
    <el-button style="float: right; padding: 3px 0" type="text">操作</el-button>
  </div>
  <div>卡片内容</div>
</el-card>
```

官网：https://element.eleme.cn/#/zh-CN/component/card

---

### Carousel 走马灯

**核心属性（el-carousel）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| height | 高度 | string | — |
| initial-index | 初始索引 | number | 0 |
| trigger | 触发方式 | string | hover/click | hover |
| autoplay | 自动播放 | boolean | true |
| interval | 间隔 | number | 3000 |
| indicator-position | 指示器位置 | string | outside/none | — |
| arrow | 箭头 | string | always/hover/never | hover |
| type | 类型 | string | card | — |
| loop | 循环 | boolean | true |
| direction | 方向 | string | vertical/horizontal | horizontal |
| pause-autoplay-on-hover | 悬停暂停 | boolean | true |

**核心属性（el-carousel-item）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| label | 标识 | string | — |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| change | 切换时 | oldIndex, newIndex |

**示例**

```html
<el-carousel height="300px" :interval="5000" arrow="always">
  <el-carousel-item v-for="item in banners" :key="item.id">
    <img :src="item.url" alt="">
  </el-carousel-item>
</el-carousel>
```

官网：https://element.eleme.cn/#/zh-CN/component/carousel

---

### Timeline 时间线

**核心属性（el-timeline）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| reverse | 反向 | boolean | false |

**核心属性（el-timeline-item）**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| timestamp | 时间戳 | string | — |
| hide-timestamp | 隐藏时间 | boolean | false |
| placement | 位置 | string | top/bottom | bottom |
| type | 节点类型 | string | primary/success/warning/danger/info | — |
| size | 尺寸 | string | normal/large | normal |
| color | 颜色 | string | — |
| icon | 图标 | string | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| — | 默认（内容） |
| dot | 自定义节点 |

**示例**

```html
<el-timeline>
  <el-timeline-item timestamp="2024-01-01" type="success" placement="top">
    创建项目
  </el-timeline-item>
  <el-timeline-item timestamp="2024-02-01" type="primary" placement="top">
    开发阶段
  </el-timeline-item>
  <el-timeline-item timestamp="2024-03-01" type="warning" placement="top">
    测试阶段
  </el-timeline-item>
</el-timeline>
```

官网：https://element.eleme.cn/#/zh-CN/component/timeline

---

### Divider 分割线

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| direction | 方向 | string | horizontal/vertical | horizontal |
| content-position | 文字位置 | string | left/right/center | left |
| border-style | 样式 | string | double/dashed/dotted/solid | solid |

**示例**

```html
<el-divider></el-divider>
<el-divider content-position="left">左侧</el-divider>
<el-divider direction="vertical"></el-divider>
```

官网：https://element.eleme.cn/#/zh-CN/component/divider

---

### Backtop 回到顶部

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| target | 触发对象 | string | — |
| visibility-height | 可见高度 | number | 200 |
| right | 右偏移 | number | 40 |
| bottom | 底偏移 | number | 40 |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| click | 点击时 | — |

**示例**

```html
<el-backtop target=".el-main" :visibility-height="300" :right="50" :bottom="50">
  <div>UP</div>
</el-backtop>
```

官网：https://element.eleme.cn/#/zh-CN/component/backtop

---

### Drawer 抽屉

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| visible / .sync | 显示 | boolean | false |
| title | 标题 | string | — |
| direction | 方向 | string | rtl/ltr/ttb/btt | rtl |
| size | 尺寸 | string/number | 30% |
| with-header | 显示标题 | boolean | true |
| modal | 遮罩 | boolean | true |
| modal-append-to-body | 遮罩插入 body | boolean | true |
| append-to-body | 自身插入 body | boolean | false |
| lock-scroll | 锁定滚动 | boolean | true |
| custom-class | 自定义类 | string | — |
| close-on-press-escape | ESC 关闭 | boolean | true |
| show-close | 显示关闭 | boolean | true |
| before-close | 关闭前 | function | — |
| destroy-on-close | 关闭销毁 | boolean | false |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| open | 打开 | — |
| opened | 打开结束 | — |
| close | 关闭 | — |
| closed | 关闭结束 | — |

**插槽**

| 插槽名 | 说明 |
|--------|------|
| — | 默认（内容） |
| title | 标题 |

**示例**

```html
<el-drawer title="详情" :visible.sync="drawerVisible" direction="rtl" size="40%">
  <div>抽屉内容</div>
</el-drawer>
```

官网：https://element.eleme.cn/#/zh-CN/component/drawer

---

### Calendar 日历

**核心属性**

| 参数 | 说明 | 类型 | 默认值 |
|------|------|------|--------|
| value / v-model | 绑定值 | Date | — |
| range | 范围 | array | — |
| first-day-of-week | 周起始 | number | 1 |

**事件**

| 事件名 | 说明 | 参数 |
|--------|------|------|
| select-date | 选择日期 | date |

**插槽**

| 插槽名 | 说明 | 参数 |
|--------|------|------|
| dateCell | 自定义单元格 | { date, data } |

**示例**

```html
<el-calendar v-model="calendarDate">
  <template slot="dateCell" slot-scope="{ data }">
    <div :class="data.isSelected ? 'is-selected' : ''">
      {{ data.day.split('-').slice(2).join('') }}
      <span v-if="data.isSelected">✔️</span>
    </div>
  </template>
</el-calendar>
```

官网：https://element.eleme.cn/#/zh-CN/component/calendar

---

## 补充说明

以上为 Element UI 全量组件的精简速查手册，每个组件保留了最核心的属性、事件、方法和插槽信息。
对于复杂组件（如 Table、Form、Cascader、Upload），建议结合官网示例深入查阅。

如果需要生成包含多个组件的完整页面，请参考 `assets/` 目录下的模板文件：

- `crud-page.vue`：CRUD 列表页（Avue 布局 + 原生 Element）
- `form-page.vue`：表单页
- `detail-page.vue`：详情页