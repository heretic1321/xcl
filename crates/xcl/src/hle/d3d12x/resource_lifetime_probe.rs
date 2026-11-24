//! A cache-owned resource is a COM child and keeps its device alive: a `Release` of the last
//! outside reference then returns 1 and a teardown after that call never runs. These run on a
//! real adapter and check that the release hook tears the cache down first, so the device still
//! reaches 0. Adapted from `@WebIdentity1`'s report in #292; needs a GPU (skips without one).
use super::*;

#[test]
fn native_cached_resource_device_teardown() {
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let sel = crate::gpu::adapter::Selector::from_env_or("auto");
        let Some((adapter, desc)) = crate::gpu::adapter::candidates(&factory, &sel)
            .into_iter()
            .next()
        else {
            return crate::gpu::recomp::runner::skip_test("no GPU");
        };
        println!("adapter={}", crate::gpu::adapter::name(&desc));
        for cached in [false, true] {
            let dev = crate::gpu::agility::create_device(&adapter, D3D_FEATURE_LEVEL_12_0).unwrap();
            let raw = dev.as_raw();
            let add: RefFn = std::mem::transmute(*vt(raw).add(1));
            let release: RefFn = std::mem::transmute(*vt(raw).add(2));
            add(raw);
            let before = release(raw);
            let rd = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: 65536,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            let mut resource: Option<ID3D12Resource> = None;
            dev.CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &rd,
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )
            .unwrap();
            let resource = resource.unwrap();
            add(raw);
            let with_child = release(raw);
            // the same adoption and release hooks production uses
            adopt(resource.as_raw(), Kind::Resource);
            adopt(raw, Kind::Device);
            assert_eq!(*vt(raw).add(2), dev_release as usize);
            let key = (raw as usize, 0x10000, [0u8; 0x38]);
            if cached {
                MADE.lock()
                    .unwrap()
                    .get_or_insert_with(ResourceAtCache::default)
                    .insert(key, resource.into_raw() as usize);
            } else {
                drop(resource);
                super::super::late_release::drain_known(true);
            }
            let raw = dev.into_raw(); // the only application-owned reference
            let remaining_refs = refs(raw);
            let n = super::super::late_release::drain_known(true);
            let _ = n;
            let cache_retained = MADE
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|c| c.get(&key).is_some());
            println!(
                "cached={cached} device_refs_before_child={before} with_child={with_child} before_release={remaining_refs} cache_retained={cache_retained}"
            );
            let after = dev_release(raw);
            println!("cached={cached} release_returned={after}");
            let cache_after = MADE
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|c| c.get(&key).is_some());
            if cache_after {
                teardown_resource_at(raw as usize);
                super::super::late_release::drain_known(true);
            }
            assert_eq!(
                after, 0,
                "the release hook frees the device even with a cached child"
            );
            assert!(
                !cache_after,
                "the cache is torn down before the final device reference goes"
            );
        }
    }
}

/// The production cache entry point, using its committed-buffer fallback outside the UMA.
/// A retail texture descriptor is not touched and no GPU work is submitted.
#[test]
fn resource_at_device_teardown_path() {
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let sel = crate::gpu::adapter::Selector::from_env_or("auto");
        let Some((adapter, desc)) = crate::gpu::adapter::candidates(&factory, &sel)
            .into_iter()
            .next()
        else {
            return crate::gpu::recomp::runner::skip_test("no GPU");
        };
        println!("adapter={}", crate::gpu::adapter::name(&desc));
        let mut rd = [0u8; 0x38];
        rd[0..4].copy_from_slice(&1u32.to_le_bytes()); // BUFFER
        rd[16..24].copy_from_slice(&65536u64.to_le_bytes());
        rd[24..28].copy_from_slice(&1u32.to_le_bytes());
        rd[28..30].copy_from_slice(&1u16.to_le_bytes());
        rd[30..32].copy_from_slice(&1u16.to_le_bytes());
        rd[36..40].copy_from_slice(&1u32.to_le_bytes()); // sample count
        rd[44..48].copy_from_slice(&1u32.to_le_bytes()); // ROW_MAJOR
        for explicit_teardown in [true, false] {
            let dev = crate::gpu::agility::create_device(&adapter, D3D_FEATURE_LEVEL_12_0).unwrap();
            let raw = dev.as_raw();
            adopt(raw, Kind::Device);
            let first = resource_at(raw, rd.as_ptr(), 0x10000)
                .expect("the production cache miss creates a native resource");
            let hit = resource_at(raw, rd.as_ptr(), 0x10000)
                .expect("the production cache hit returns an owned reference");
            assert_eq!(first.as_raw(), hit.as_raw());
            let key = (raw as usize, 0x10000, rd);
            assert!(MADE
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|c| c.get(&key) == Some(first.as_raw() as usize)));
            drop(first);
            drop(hit);
            if explicit_teardown {
                teardown_resource_at(raw as usize);
                super::super::late_release::drain_known(true);
            }
            let raw = dev.into_raw();
            let after = dev_release(raw);
            let cache_retained = MADE
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|c| c.get(&key).is_some());
            println!("resource_at explicit_teardown={explicit_teardown} release_returned={after} cache_retained={cache_retained}");
            if cache_retained {
                teardown_resource_at(raw as usize);
                super::super::late_release::drain_known(true);
            }
            assert!(
                !cache_retained,
                "the production resource_at cache is released with the device"
            );
            if explicit_teardown {
                assert_eq!(
                    after, 0,
                    "the explicit teardown control releases the device"
                );
            }
        }
    }
}
