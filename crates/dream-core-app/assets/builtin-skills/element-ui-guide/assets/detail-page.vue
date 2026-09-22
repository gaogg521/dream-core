<template>
  <!--
    详情页模板（描述列表布局 + 状态标签 + 时间线 + 操作按钮）
    技术栈：Vue 2 + Element UI + Options API + axios 封装
  -->
  <div class="detail-page">
    <el-card shadow="never">
      <!-- 页头 -->
      <el-page-header @back="goBack" content="详情查看" slot="header"></el-page-header>

      <el-divider></el-divider>

      <!-- 基本信息卡 -->
      <el-card shadow="hover" class="info-card">
        <div slot="header">
          <span>基本信息</span>
          <el-button type="primary" size="mini" style="float: right" @click="handleEdit">编 辑</el-button>
        </div>
        <el-descriptions :column="3" border>
          <el-descriptions-item label="名称">{{ detail.name }}</el-descriptions-item>
          <el-descriptions-item label="编号">{{ detail.code }}</el-descriptions-item>
          <el-descriptions-item label="分类">{{ detail.categoryName }}</el-descriptions-item>
          <el-descriptions-item label="状态">
            <el-tag :type="detail.status === 1 ? 'success' : 'info'" size="small">
              {{ detail.status === 1 ? '启用' : '禁用' }}
            </el-tag>
          </el-descriptions-item>
          <el-descriptions-item label="排序">{{ detail.sort }}</el-descriptions-item>
          <el-descriptions-item label="创建时间">{{ detail.createTime }}</el-descriptions-item>
        </el-descriptions>
      </el-card>

      <!-- 时间信息卡 -->
      <el-card shadow="hover" class="info-card">
        <div slot="header"><span>时间信息</span></div>
        <el-descriptions :column="2" border>
          <el-descriptions-item label="生效开始日期">{{ detail.effectiveStart }}</el-descriptions-item>
          <el-descriptions-item label="生效结束日期">{{ detail.effectiveEnd }}</el-descriptions-item>
          <el-descriptions-item label="提醒时间">{{ detail.remindTime }}</el-descriptions-item>
          <el-descriptions-item label="是否开启">
            <el-tag :type="detail.enabled ? 'success' : 'info'" size="small">
              {{ detail.enabled ? '已开启' : '未开启' }}
            </el-tag>
          </el-descriptions-item>
        </el-descriptions>
      </el-card>

      <!-- 描述信息卡 -->
      <el-card shadow="hover" class="info-card">
        <div slot="header"><span>描述信息</span></div>
        <div class="description-content">{{ detail.description || '暂无描述' }}</div>
      </el-card>

      <!-- 附件卡 -->
      <el-card shadow="hover" class="info-card">
        <div slot="header"><span>附件</span></div>
        <el-upload
          action="/api/upload"
          :file-list="detail.fileList"
          list-type="text"
          :show-upload="false"
          :on-preview="handlePreviewFile"
          disabled
        >
          <div v-if="!detail.fileList || detail.fileList.length === 0" class="empty-tip">暂无附件</div>
        </el-upload>
      </el-card>

      <!-- 标签信息 -->
      <el-card shadow="hover" class="info-card">
        <div slot="header"><span>标签</span></div>
        <div v-if="detail.tags && detail.tags.length > 0">
          <el-tag
            v-for="tag in detail.tags"
            :key="tag"
            class="tag-item"
            :type="getTagType(tag)"
            size="medium"
          >
            {{ tag }}
          </el-tag>
        </div>
        <div v-else class="empty-tip">暂无标签</div>
      </el-card>

      <!-- 审批时间线 -->
      <el-card shadow="hover" class="info-card">
        <div slot="header"><span>审批流程</span></div>
        <el-timeline>
          <el-timeline-item
            v-for="(item, index) in approvalHistory"
            :key="index"
            :timestamp="item.time"
            :type="item.type"
            placement="top"
          >
            {{ item.content }}
          </el-timeline-item>
        </el-timeline>
        <el-empty v-if="approvalHistory.length === 0" description="暂无审批记录"></el-empty>
      </el-card>

      <!-- 底部按钮 -->
      <div class="footer-buttons">
        <el-button @click="goBack">返 回</el-button>
      </div>
    </el-card>
  </div>
</template>

<script>
export default {
  name: 'DetailPage',
  data() {
    return {
      detail: {
        name: '',
        code: '',
        categoryName: '',
        status: '',
        sort: '',
        createTime: '',
        effectiveStart: '',
        effectiveEnd: '',
        remindTime: '',
        enabled: false,
        description: '',
        fileList: [],
        tags: []
      },
      approvalHistory: []
    }
  },
  created() {
    const id = this.$route.params.id
    if (id) {
      this.loadDetail(id)
    }
  },
  methods: {
    // 加载详情数据
    loadDetail(id) {
      this.$api.getDetail({ id }).then(res => {
        this.detail = Object.assign({}, this.detail, res.data)
        if (res.data.approvalHistory) {
          this.approvalHistory = res.data.approvalHistory
        }
      })
    },
    // 编辑
    handleEdit() {
      this.$router.push(`/edit/${this.$route.params.id}`)
    },
    // 预览文件
    handlePreviewFile(file) {
      if (file.url) {
        window.open(file.url, '_blank')
      }
    },
    // 标签颜色
    getTagType(tag) {
      const types = ['primary', 'success', 'warning', 'danger', 'info']
      return types[tag.charCodeAt(0) % types.length]
    },
    // 返回
    goBack() {
      this.$router.go(-1)
    }
  }
}
</script>

<style>
.detail-page {
  padding: 20px;
}
.info-card {
  margin-bottom: 20px;
}
.description-content {
  line-height: 1.8;
  color: #606266;
  white-space: pre-wrap;
}
.empty-tip {
  color: #909399;
  text-align: center;
  padding: 20px 0;
}
.tag-item {
  margin-right: 8px;
  margin-bottom: 8px;
}
.footer-buttons {
  text-align: center;
  padding-top: 20px;
}
</style>
