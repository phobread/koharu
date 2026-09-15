'use client'

import { EyeIcon, SlidersHorizontalIcon, SparklesIcon, TypeIcon } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { AiPanel } from '@/components/panels/AiPanel'
import { LayersPanel } from '@/components/panels/LayersPanel'
import { RenderControlsPanel } from '@/components/panels/RenderControlsPanel'
import { TextBlocksPanel } from '@/components/panels/TextBlocksPanel'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useGetCodexAuthStatus } from '@/lib/api/default/default'

export function Panels() {
  const { t } = useTranslation()
  const { data: codexAuth } = useGetCodexAuthStatus()
  const codexSignedIn = codexAuth?.signedIn === true
  const [panel, setPanel] = useState('text')

  useEffect(() => {
    if (!codexSignedIn && panel === 'ai') setPanel('text')
  }, [codexSignedIn, panel])

  return (
    <aside
      className='flex h-full min-h-0 w-full flex-col bg-[var(--surface-panel)]'
      aria-label={t('panels.inspector', 'Inspector')}
    >
      <Tabs
        value={panel}
        onValueChange={setPanel}
        className='h-full min-h-0 gap-0'
        data-testid='panels-work-tabs'
      >
        <TabsList
          className='h-auto! min-h-11 w-full shrink-0 flex-wrap gap-1 rounded-none border-b border-border/80 bg-transparent px-2.5 py-1.5'
          aria-label={t('panels.inspector', 'Inspector')}
        >
          <TabsTrigger
            value='text'
            data-testid='panels-tab-textblocks'
            className='h-8 gap-1.5 px-2 text-xs'
          >
            <TypeIcon className='size-3.5' />
            {t('panels.text', 'Text')}
          </TabsTrigger>
          <TabsTrigger
            value='layout'
            data-testid='panels-tab-layout'
            className='h-8 gap-1.5 px-2 text-xs'
          >
            <SlidersHorizontalIcon className='size-3.5' />
            {t('panels.properties', 'Properties')}
          </TabsTrigger>
          <TabsTrigger
            value='layers'
            data-testid='panels-tab-layers'
            className='h-8 gap-1.5 px-2 text-xs'
          >
            <EyeIcon className='size-3.5' />
            {t('layers.title')}
          </TabsTrigger>
          {codexSignedIn && (
            <TabsTrigger
              value='ai'
              data-testid='panels-tab-ai'
              className='h-8 gap-1.5 px-2 text-xs'
            >
              <SparklesIcon className='size-3.5' />
              {t('panels.ai')}
            </TabsTrigger>
          )}
        </TabsList>

        {/* Keep editing panes mounted so tab changes retain local drafts and selection. */}
        <TabsContent
          forceMount
          value='text'
          className='flex min-h-0 flex-1 data-[state=inactive]:hidden'
          data-testid='panels-textblocks-tab'
        >
          <TextBlocksPanel />
        </TabsContent>
        <TabsContent
          forceMount
          value='layout'
          className='min-h-0 flex-1 data-[state=inactive]:hidden'
          data-testid='panels-layout'
        >
          <ScrollArea className='h-full' viewportClassName='[&>div]:!block'>
            <div className='p-3'>
              <RenderControlsPanel />
            </div>
          </ScrollArea>
        </TabsContent>
        <TabsContent
          forceMount
          value='layers'
          className='min-h-0 flex-1 data-[state=inactive]:hidden'
          data-testid='panels-layers'
        >
          <ScrollArea className='h-full'>
            <div className='p-2'>
              <LayersPanel />
            </div>
          </ScrollArea>
        </TabsContent>
        {codexSignedIn && (
          <TabsContent
            forceMount
            value='ai'
            className='min-h-0 flex-1 data-[state=inactive]:hidden'
            data-testid='panels-ai'
          >
            <ScrollArea className='h-full' viewportClassName='[&>div]:!block'>
              <div className='p-3'>
                <AiPanel />
              </div>
            </ScrollArea>
          </TabsContent>
        )}
      </Tabs>
    </aside>
  )
}
