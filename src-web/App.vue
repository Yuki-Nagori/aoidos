<script setup lang="ts">
import { computed } from "vue";
import { useProduct } from "./composables/useProduct";
import { useGameView } from "./composables/useGameView";
import { useGameInput } from "./composables/useGameInput";
import { useGameControls } from "./composables/useGameControls";

const product = useProduct();
const sessionId = computed(() => product.session.value?.sessionId);
const game = useGameView(sessionId);
const { phase, turn, records } = game;
const current = computed(() => phase.state.value.snapshot);
const input = useGameInput(sessionId);
const controls = useGameControls(current);
const error = computed(
  () =>
    product.error.value ??
    input.error.value ??
    controls.error.value ??
    current.value?.lastOperation?.error ??
    turn.view.value.snapshot?.error ??
    phase.state.value.error ??
    records.state.value.error ??
    turn.view.value.recoveryError ??
    phase.connectionError.value ??
    records.connectionError.value ??
    turn.connectionError.value,
);
const committed = computed(() => game.latestPublicRecord.value?.body);
const preview = computed(() =>
  turn.view.value.snapshot?.turnId === game.latestPublicRecord.value?.turnId
    ? undefined
    : turn.view.value.snapshot,
);
const selectedScript = computed(() =>
  product.scripts.value.find((script) => script.scriptId === product.scriptId.value),
);
void product.load();
</script>

<template>
  <main class="mx-auto grid min-h-screen max-w-4xl gap-6 p-6 text-ink">
    <section class="glass-panel">
      <h1 class="text-2xl font-semibold">Aoidos</h1>
      <p class="text-sm text-muted">选择剧本与模型后开始。重开周目不会自动生成。</p>
      <label
        >模型
        <select v-model="product.model.value" aria-label="模型" :disabled="product.busy.value">
          <option v-for="model in product.models" :key="model.id" :value="model.id">
            {{ model.label }}
          </option>
        </select></label
      >
      <button :disabled="product.busy.value" @click="product.configureKey">
        设置 API 密钥（系统窗口）
      </button>
      <p v-if="product.keyStatus.value">
        {{ product.keyStatus.value.set ? "密钥已设置" : "密钥未设置" }}
      </p>
      <label
        >剧本
        <select v-model="product.scriptId.value" :disabled="product.busy.value">
          <option
            v-for="script in product.scripts.value"
            :key="script.scriptId"
            :value="script.scriptId"
          >
            {{ script.title }}
          </option>
        </select></label
      >
      <div class="flex gap-4">
        <button :disabled="product.busy.value" @click="product.open(false)">重开 / 首次打开</button
        ><button :disabled="product.busy.value" @click="product.open(true)">新建周目</button
        ><button @click="game.reconnect">重新连接</button>
      </div>
      <details v-if="selectedScript">
        <summary>剧本署名与许可</summary>
        <pre class="whitespace-pre-wrap text-sm">{{ selectedScript.attributions }}</pre>
      </details>
      <p v-if="error" role="alert">{{ error.code }}：{{ error.message }}</p>
    </section>
    <section v-if="product.session.value" class="glass-panel">
      <h2>{{ product.session.value.title }}</h2>
      <p>阶段：{{ current?.phase ?? "正在读取" }}</p>
      <p v-if="current?.resumeRequired">已暂停，继续需要显式恢复。</p>
      <p v-if="game.historyPending.value">记录尚未确认，请重新连接。</p>
      <ol v-else>
        <li v-for="item in records.items.value" :key="item.recordSeq" class="whitespace-pre-wrap">
          <span>{{ item.kind }}：</span
          ><span v-if="item.body && 'text' in item.body">{{ item.body.text }}</span
          ><button v-else-if="item.bodyRef" @click="records.loadBody(item.bodyRef)">
            读取完整正文</button
          ><span v-else>{{ item.body }}</span>
        </li>
      </ol>
      <pre v-if="preview" class="whitespace-pre-wrap" data-testid="live-text">{{
        preview.text
      }}</pre>
      <pre
        v-if="!game.historyPending.value && records.state.value.body !== undefined"
        class="whitespace-pre-wrap"
        >{{ records.state.value.body }}</pre>
      <p v-if="committed && 'outcome' in committed">
        生成：{{ committed.outcome }}
        <span v-if="'finishReason' in committed">{{ committed.finishReason }}</span>
      </p>
      <p v-else-if="game.latestPublicRecord.value?.outcome">
        生成：{{ game.latestPublicRecord.value.outcome }}
      </p>
      <p v-else-if="preview?.outcome">生成：{{ preview.outcome }} {{ preview.finishReason }}</p>
      <form class="flex gap-2" @submit.prevent="input.send">
        <textarea
          v-model="input.draft.value"
          aria-label="行动"
          class="w-full border border-hairline bg-transparent p-2"
          @compositionstart="input.composing.value = true"
          @compositionend="input.composing.value = false"
        />
        <button
          type="submit"
          :disabled="
            product.busy.value ||
            input.busy.value ||
            !current ||
            current.phase !== 'idle' ||
            current.resumeRequired
          "
        >
          提交行动
        </button>
      </form>
      <div class="flex gap-4">
        <button
          v-if="current?.inFlight"
          :disabled="controls.busy.value"
          @click="controls.control('cancel')"
        >
          取消回合</button
        ><button
          v-if="current?.resumeRequired"
          :disabled="controls.busy.value"
          @click="controls.control('resume')"
        >
          恢复</button
        ><button
          v-if="current?.check?.status === 'waiting' && current.inFlight?.roundId"
          :disabled="controls.busy.value"
          @click="controls.control('check')"
        >
          确认判定（{{ current.check.expression }}）
        </button>
      </div>
    </section>
  </main>
</template>
