use std::{mem, panic::Location, sync::Arc};

use anyhow::Ok;
use gpu_allocator::{d3d12::Allocator, MemoryLocation};
use parking_lot::Mutex;
use tracy_client::GpuSpan;
use windows::Win32::Graphics::{Direct3D11::*, Direct3D12::*};

use crate::{resource::Buffer, util::ID3D12ObjectExt};

const QUERY_PAIRS: u32 = 32;
const QUERY_COUNT: u32 = QUERY_PAIRS * 2;
const QUERY_BUFFER_SIZE: u64 = QUERY_COUNT as u64 * std::mem::size_of::<u64>() as u64;

pub struct QueryContexts {
    pub graphics: tracy_client::GpuContext,
    pub copy: tracy_client::GpuContext,
}

impl QueryContexts {
    pub fn new(
        graphics_queue: &ID3D12CommandQueue,
        copy_queue: &ID3D12CommandQueue,
    ) -> anyhow::Result<Self> {
        unsafe {
            let mut cpu_timestamp = 0;
            let mut graphics_timestamp_value = 0;
            graphics_queue
                .GetClockCalibration(&mut graphics_timestamp_value, &mut cpu_timestamp)?;
            let graphics = tracy_client::Client::running().unwrap().new_gpu_context(
                Some("Graphics Queue"),
                tracy_client::GpuContextType::Direct3D12,
                graphics_timestamp_value as i64,
                1_000_000_000.0 / graphics_queue.GetTimestampFrequency().unwrap() as f32,
            )?;

            let mut copy_timestamp_value = 0;
            copy_queue.GetClockCalibration(&mut copy_timestamp_value, &mut cpu_timestamp)?;

            let copy = tracy_client::Client::running().unwrap().new_gpu_context(
                Some("Copy Queue"),
                tracy_client::GpuContextType::Direct3D12,
                copy_timestamp_value as i64,
                1_000_000_000.0 / copy_queue.GetTimestampFrequency().unwrap() as f32,
            )?;

            Ok(Self { graphics, copy })
        }
    }
}

pub struct QueryManager {
    query_pool: ID3D12QueryHeap,
    command_list: ID3D12GraphicsCommandList,
    query_context: tracy_client::GpuContext,
    tracy_spans: Vec<GpuSpan>,
    buffer: Buffer,
}

impl QueryManager {
    pub fn new(
        device: &ID3D12Device,
        allocator: &Arc<Mutex<Allocator>>,
        command_list: ID3D12GraphicsCommandList,
        query_context: tracy_client::GpuContext,
        label: &str,
    ) -> anyhow::Result<Self> {
        let query_type = match unsafe { command_list.GetType() } {
            D3D12_COMMAND_LIST_TYPE_DIRECT | D3D12_COMMAND_LIST_TYPE_COMPUTE => {
                D3D12_QUERY_HEAP_TYPE_TIMESTAMP
            }
            D3D12_COMMAND_LIST_TYPE_COPY => D3D12_QUERY_HEAP_TYPE_COPY_QUEUE_TIMESTAMP,
            _ => unreachable!(),
        };

        let query_pool = unsafe {
            let mut query_pool: Option<ID3D12QueryHeap> = None;

            device.CreateQueryHeap(
                &D3D12_QUERY_HEAP_DESC { Type: query_type, Count: QUERY_PAIRS * 2, NodeMask: 0 },
                &mut query_pool,
            )?;

            query_pool.unwrap()
        };
        query_pool.set_name(&format!("{label}: Query Pool"));

        let buffer = Buffer::new(
            device,
            allocator,
            MemoryLocation::GpuToCpu,
            QUERY_BUFFER_SIZE,
            false,
            &format!("{label}: Query Buffer"),
        )?;

        Ok(Self { query_pool, command_list, query_context, tracy_spans: Vec::new(), buffer })
    }

    #[track_caller]
    pub fn start_span(&mut self, name: &str) -> TimestampSpan {
        let location = Location::caller();
        unsafe {
            let start_timestamp_index = self.tracy_spans.len() as u32 * 2;
            let end_timestamp_index = start_timestamp_index + 1;

            let span =
                self.query_context.span_alloc(name, "", location.file(), location.line()).unwrap();

            self.command_list.EndQuery(
                &self.query_pool,
                D3D12_QUERY_TYPE_TIMESTAMP,
                start_timestamp_index,
            );

            self.tracy_spans.push(span);

            TimestampSpan { end: end_timestamp_index }
        }
    }

    pub fn end_span(&mut self, span: TimestampSpan) {
        unsafe {
            self.command_list.EndQuery(&self.query_pool, D3D12_QUERY_TYPE_TIMESTAMP, span.end);

            self.tracy_spans[span.end as usize / 2].end_zone();
        }
    }

    pub fn resolve(&mut self) {
        unsafe {
            self.command_list.ResolveQueryData(
                &self.query_pool,
                D3D12_QUERY_TYPE_TIMESTAMP,
                0,
                self.tracy_spans.len() as u32 * 2,
                self.buffer.resource(),
                0,
            );
        }
    }

    pub fn read(&mut self) {
        let mapping: &[u64] = bytemuck::cast_slice(self.buffer.mapping());
        for (i, span) in self.tracy_spans.drain(..).enumerate() {
            let start = mapping[i * 2];
            let end = mapping[i * 2 + 1];
            span.upload_timestamp(start as i64, end as i64);
        }
    }
}

pub struct TimestampSpan {
    end: u32,
}

pub fn create_d3d11_context(queue: &ID3D12CommandQueue) -> tracy_client::GpuContext {
    unsafe {
        let mut gpu_timestamp = 0;
        let mut cpu_timestamp = 0;
        queue.GetClockCalibration(&mut gpu_timestamp, &mut cpu_timestamp).unwrap();
        let d3d12_frequency = queue.GetTimestampFrequency().unwrap();

        let period = (1_000_000_000 as f64 / d3d12_frequency as f64) as f32;

        tracy_client::Client::running()
            .unwrap()
            .new_gpu_context(
                Some("D3D11 Present Queue"),
                tracy_client::GpuContextType::Direct3D11,
                gpu_timestamp as i64,
                period,
            )
            .unwrap()
    }
}

pub unsafe fn d3d11_query_get_data<T: Default + PartialEq>(
    context: &ID3D11DeviceContext,
    query: &ID3D11Query,
) -> Option<T> {
    unsafe {
        let mut data = T::default();
        context
            .GetData(query, Some(&mut data as *mut _ as *mut _), mem::size_of_val(&data) as u32, 0)
            .ok()?;
        if data == T::default() {
            return None;
        }
        Some(data)
    }
}
