import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { onMounted, ref } from 'vue'
import IndexPage from '../../app/pages/index.vue'

vi.stubGlobal('ref', ref)
vi.stubGlobal('onMounted', onMounted)
vi.stubGlobal('useRuntimeConfig', () => ({ public: { apiBaseUrl: 'http://127.0.0.1:8080' } }))
vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
  ok: true,
  json: async () => ({ items: [], nextCursor: null }),
}))

describe('index page', () => {
  it('renders the board heading', () => {
    const wrapper = mount(IndexPage)
    expect(wrapper.find('h1').text()).toBe('CW Kanban Board')
  })
})
